//! Splash modules for kernels: the stdlib and any document or AI-written
//! library, compiled the same way. A module is Splash text of top-level
//! `fn` items and constants (`let` / `const`, tables, structs); the host
//! resolves each import to its text and passes `(path, source)` pairs, and
//! the kernel inlines what it reaches, exactly like its own functions:
//! same budgets, same validation, no privileged tier.
//!
//! ```text
//! use std.curve.*                 // names in unqualified
//! use std.field as f              // f.isolines(..)
//! use lib("my/looks", "3") as looks
//! let h = curve.arclen(pts, n)    // qualified without a `use`: std modules
//! ```
//!
//! Resolution is a rewrite of the parsed items: every item of a module
//! imported as `m` is renamed `m.name`, and the module's references to its
//! own items follow (locals shadow them); `use m.*` adds unqualified copies.
//! A kernel's own items shadow imported ones; two glob imports bringing the
//! same name are an error at the second `use`. A qualified call to a std
//! module needs no `use` (`curve.sample(..)`), and the stdlib shared with
//! shaders (`std.shared`) is glob-imported by default at the lowest
//! priority (below the kernel prelude).

use crate::parse::*;
use crate::ShaderError;
use std::collections::{HashMap, HashSet};

/// A module's text as the host resolved it. `path` is what a `use` names:
/// `std.noise`, or `lib:id@rev` for `use lib("id", "rev")`.
#[derive(Clone, Copy, Debug)]
pub struct Module<'a> {
    pub path: &'a str,
    pub source: &'a str,
}

/// The stdlib shared with shaders and documents (SX1, SPLASH-TOKENS §5):
/// written in the subset every Splash compiler takes. The shader compiler
/// includes this text; kernels import it as `std.shared` (in scope by
/// default like every std module).
pub const SHARED_STD: &str = include_str!("../std/shared.splash");

/// The kernel stdlib modules that ship (see std/*.splash).
pub const STD: &[(&str, &str)] = &[
    ("std.shared", SHARED_STD),
    ("std.search", include_str!("../std/search.splash")),
    ("std.curve", include_str!("../std/curve.splash")),
    ("std.field", include_str!("../std/field.splash")),
    ("std.poly", include_str!("../std/poly.splash")),
    ("std.ease", include_str!("../std/ease.splash")),
    ("std.anim", include_str!("../std/anim.splash")),
    ("std.cam", include_str!("../std/cam.splash")),
    ("std.beat", include_str!("../std/beat.splash")),
];

/// The std modules in scope unqualified by default (the kernel modules'
/// names are generic, so they are reached qualified, `curve.sample(..)`,
/// or through an explicit `use std.curve.*`).
const DEFAULT_GLOB: &[&str] = &["std.shared"];

/// Nesting of modules importing modules.
const MAX_DEPTH: usize = 8;

struct Resolver<'a> {
    host: &'a [Module<'a>],
    /// Where the next module's spans start (past the user code and the
    /// prelude, so errors inside modules are reported at the user's call).
    next_base: usize,
    stack: Vec<String>,
}

impl<'a> Resolver<'a> {
    fn source(&self, path: &str) -> Option<&'a str> {
        if let Some(m) = self.host.iter().find(|m| m.path == path) {
            return Some(m.source);
        }
        let std_path = if path.starts_with("std.") { path.to_string() } else { format!("std.{}", path) };
        STD.iter().find(|(p, _)| *p == std_path).map(|(_, s)| *s)
    }

    /// Parses a module's text with spans placed past everything so far.
    fn parse(&mut self, path: &str, src: &str, at: Span) -> Result<Vec<Item>, ShaderError> {
        let fail = |e: ShaderError| {
            let (line, col) = e.line_col(src);
            ShaderError::new(at.start, at.end.max(at.start + 1), format!("in module `{}` (line {}, column {}): {}", path, line, col, e.message))
        };
        let mut toks = lex(src).map_err(fail)?;
        let base = self.next_base;
        for t in &mut toks {
            t.start += base;
            t.end += base;
        }
        self.next_base += src.len() + 1;
        Parser::new(&toks).items().map_err(|e| {
            let e = ShaderError::new(e.start - base.min(e.start), e.end - base.min(e.end), e.message);
            fail(e)
        })
    }

    /// Resolves the `use` items of `items`: returns the imported items
    /// (qualified, plus glob copies) and `items` without its uses.
    fn resolve(&mut self, items: Vec<Item>, auto_std: bool, keep: &HashSet<String>) -> Result<Vec<Item>, ShaderError> {
        let own: HashSet<String> = items.iter().filter(|i| !matches!(i, Item::Use { .. })).map(|i| i.name().to_string()).collect();
        let mut uses: Vec<(String, bool, String, Span)> = Vec::new();
        for it in &items {
            if let Item::Use { path, glob, alias, span } = it {
                let alias = match alias {
                    Some(a) => a.clone(),
                    None if path.starts_with("lib:") && !*glob => return Err(ShaderError::new(span.start, span.end, "name a library module: `use lib(\"id\", \"rev\") as name`".into())),
                    None => path.rsplit('.').next().unwrap_or(path).to_string(),
                };
                uses.push((path.clone(), *glob, alias, *span));
            }
        }
        // Qualified calls to modules nobody imported: std paths or host paths
        // written out in full (`std.curve.arclen(..)`, `curve.arclen(..)`).
        let aliases: HashSet<String> = uses.iter().map(|u| u.2.clone()).collect();
        let mut called = HashSet::new();
        for it in &items {
            if let Item::Fn(f) = it {
                qualified_calls(&f.body, &mut called);
            }
        }
        let mut called: Vec<String> = called.into_iter().collect();
        called.sort();
        for q in called {
            if !aliases.contains(&q) && !uses.iter().any(|u| u.0 == q) && self.source(&q).is_some() {
                uses.push((q.clone(), false, q.clone(), Span { start: 0, end: 1 }));
            }
        }
        let mut out: Vec<Item> = Vec::new();
        let mut exported: HashMap<String, String> = HashMap::new();
        let mut have: HashSet<String> = HashSet::new();
        let mut aliases: HashSet<String> = HashSet::new();
        for (path, glob, alias, span) in &uses {
            let m = self.import(path, alias, *span)?;
            aliases.insert(alias.clone());
            for it in &m {
                if have.insert(it.name().to_string()) {
                    out.push(it.clone());
                }
            }
            if *glob {
                self.globs(&m, alias, path, &own, keep, &mut exported, &mut out, &mut have, false, *span)?;
            }
        }
        if auto_std {
            // `use std.*` at the lowest priority: a name the kernel, an import
            // or the prelude has already is not replaced.
            for (path, _) in STD.iter().filter(|(p, _)| DEFAULT_GLOB.contains(p)) {
                let alias = path.rsplit('.').next().unwrap().to_string();
                if aliases.contains(&alias) {
                    continue;
                }
                let span = Span { start: 0, end: 1 };
                let m = self.import(path, &alias, span)?;
                for it in &m {
                    if have.insert(it.name().to_string()) {
                        out.push(it.clone());
                    }
                }
                self.globs(&m, &alias, path, &own, keep, &mut exported, &mut out, &mut have, true, span)?;
            }
        }
        // References to module constants written `m.NAME` (a field of a
        // name that is not a value) become the qualified name.
        let mut user: Vec<Item> = items.into_iter().filter(|i| !matches!(i, Item::Use { .. })).collect();
        let quals: HashSet<String> = have.iter().filter(|n| n.contains('.')).cloned().collect();
        for it in &mut user {
            qualify_fields(it, &aliases, &quals);
        }
        out.extend(user);
        Ok(out)
    }

    /// Unqualified copies of a module's items for `use m.*`.
    #[allow(clippy::too_many_arguments)]
    fn globs(
        &self,
        m: &[Item],
        alias: &str,
        path: &str,
        own: &HashSet<String>,
        keep: &HashSet<String>,
        exported: &mut HashMap<String, String>,
        out: &mut Vec<Item>,
        have: &mut HashSet<String>,
        quiet: bool,
        span: Span,
    ) -> Result<(), ShaderError> {
        let prefix = format!("{}.", alias);
        for it in m {
            let Some(short) = it.name().strip_prefix(&prefix) else { continue };
            // Only the module's own items (not what it imported itself).
            if short.contains('.') || own.contains(short) || (quiet && keep.contains(short)) {
                continue;
            }
            if let Some(other) = exported.get(short) {
                if quiet || other == path {
                    continue;
                }
                return Err(ShaderError::new(span.start, span.end, format!("`use {}.*` brings `{}`, which `use {}.*` brought already: import one of them by name", path, short, other)));
            }
            if !have.insert(short.to_string()) {
                continue;
            }
            exported.insert(short.to_string(), path.to_string());
            let mut copy = it.clone();
            rename(&mut copy, short.to_string());
            out.push(copy);
        }
        Ok(())
    }

    /// A module's items, resolved and qualified by `alias`.
    fn import(&mut self, path: &str, alias: &str, span: Span) -> Result<Vec<Item>, ShaderError> {
        if self.stack.iter().any(|p| p == path) || self.stack.len() >= MAX_DEPTH {
            return Err(ShaderError::new(span.start, span.end, format!("module `{}` imports itself (through {})", path, self.stack.join(" -> "))));
        }
        let Some(src) = self.source(path) else {
            let hint = if path.starts_with("lib:") { " (the host did not resolve this library revision)" } else { "" };
            return Err(ShaderError::new(span.start, span.end.max(span.start + 1), format!("no module `{}`{}", path, hint)));
        };
        let items = self.parse(path, src, span)?;
        if items.iter().any(|i| matches!(i, Item::Var { .. })) {
            return Err(ShaderError::new(span.start, span.end, format!("module `{}` declares a `var`: modules hold functions and constants", path)));
        }
        self.stack.push(path.to_string());
        let resolved = self.resolve(items, false, &HashSet::new());
        self.stack.pop();
        let resolved = resolved?;
        // Qualify everything the module now defines by the alias.
        let names: HashSet<String> = resolved.iter().map(|i| i.name().to_string()).collect();
        let mut out = Vec::with_capacity(resolved.len());
        for mut it in resolved {
            let q = format!("{}.{}", alias, it.name());
            qualify(&mut it, alias, &names);
            rename(&mut it, q);
            out.push(it);
        }
        Ok(out)
    }
}

/// Resolves a kernel's imports. `user_len` is the length of the kernel's
/// source plus everything already placed after it (the prelude), so module
/// spans land past both; `prelude` names are kept over default std globs.
pub fn resolve(items: Vec<Item>, next_base: usize, host: &[Module], prelude: &[Item]) -> Result<Vec<Item>, ShaderError> {
    let mut r = Resolver { host, next_base, stack: Vec::new() };
    let keep: HashSet<String> = prelude.iter().map(|i| i.name().to_string()).collect();
    r.resolve(items, true, &keep)
}

fn rename(it: &mut Item, name: String) {
    match it {
        Item::Let { name: n, .. } | Item::Var { name: n, .. } | Item::Struct { name: n, .. } => *n = name,
        Item::Fn(f) => f.name = name,
        Item::Use { .. } => {}
    }
}

/// Rewrites a module item's references to the module's own `names` as
/// `alias.name` (names bound locally shadow them).
fn qualify(it: &mut Item, alias: &str, names: &HashSet<String>) {
    let q = |n: &str| format!("{}.{}", alias, n);
    let mut w = Walk { map: &|n: &str| names.contains(n).then(|| q(n)), fields: None, scopes: Vec::new() };
    match it {
        Item::Let { value, .. } | Item::Var { value, .. } => w.expr(value),
        Item::Struct { fields, .. } => {
            for (_, e) in fields {
                w.expr(e);
            }
        }
        Item::Fn(f) => {
            w.scopes.push(f.params.iter().map(|p| p.0.clone()).collect());
            w.stmts(&mut f.body);
        }
        Item::Use { .. } => {}
    }
}

/// `m.NAME` (a field of an unbound alias) -> the qualified name.
fn qualify_fields(it: &mut Item, aliases: &HashSet<String>, quals: &HashSet<String>) {
    let mut w = Walk { map: &|_: &str| None, fields: Some((aliases, quals)), scopes: Vec::new() };
    match it {
        Item::Let { value, .. } | Item::Var { value, .. } => w.expr(value),
        Item::Struct { fields, .. } => {
            for (_, e) in fields {
                w.expr(e);
            }
        }
        Item::Fn(f) => {
            w.scopes.push(f.params.iter().map(|p| p.0.clone()).collect());
            w.stmts(&mut f.body);
        }
        Item::Use { .. } => {}
    }
}

struct Walk<'m> {
    map: &'m dyn Fn(&str) -> Option<String>,
    fields: Option<(&'m HashSet<String>, &'m HashSet<String>)>,
    scopes: Vec<HashSet<String>>,
}

impl Walk<'_> {
    fn bound(&self, n: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(n))
    }

    fn bind(&mut self, n: &str) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(n.to_string());
        }
    }

    fn stmts(&mut self, b: &mut [Stmt]) {
        self.scopes.push(HashSet::new());
        for s in b {
            match s {
                Stmt::Let { name, value, .. } => {
                    self.expr(value);
                    let n = name.clone();
                    self.bind(&n);
                }
                Stmt::Assign { target, value, .. } => {
                    self.expr(target);
                    self.expr(value);
                }
                Stmt::Expr(e) => self.expr(e),
                Stmt::For { var, from, to, body, .. } => {
                    self.expr(from);
                    self.expr(to);
                    self.scopes.push([var.clone()].into_iter().collect());
                    self.stmts(body);
                    self.scopes.pop();
                }
                Stmt::While { cond, body, .. } => {
                    self.expr(cond);
                    self.stmts(body);
                }
                Stmt::Loop { body, .. } => self.stmts(body),
                Stmt::Return(Some(e), _) => self.expr(e),
                Stmt::Return(None, _) | Stmt::Break(_) | Stmt::Continue(_) => {}
            }
        }
        self.scopes.pop();
    }

    fn expr(&mut self, e: &mut Expr) {
        match &mut e.kind {
            ExprKind::Ident(n) => {
                if !self.bound(n) {
                    if let Some(q) = (self.map)(n) {
                        *n = q;
                    }
                }
            }
            ExprKind::Call(n, args) => {
                if !self.bound(n) {
                    if let Some(q) = (self.map)(n) {
                        *n = q;
                    }
                }
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::StructLit(n, fields) => {
                if let Some(q) = (self.map)(n) {
                    *n = q;
                }
                for (_, v) in fields {
                    self.expr(v);
                }
            }
            ExprKind::Field(base, name) => {
                if let (Some((aliases, quals)), ExprKind::Ident(a)) = (self.fields, &base.kind) {
                    let q = format!("{}.{}", a, name);
                    if aliases.contains(a) && !self.bound(a) && quals.contains(&q) {
                        e.kind = ExprKind::Ident(q);
                        return;
                    }
                }
                self.expr(base);
            }
            ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Neg(a) | ExprKind::Not(a) => self.expr(a),
            ExprKind::If(arms, else_) => {
                for (c, b) in arms {
                    self.expr(c);
                    self.stmts(b);
                }
                if let Some(b) = else_ {
                    self.stmts(b);
                }
            }
            ExprKind::Match(s, arms) => {
                self.expr(s);
                for (pats, b) in arms {
                    for p in pats.iter_mut().flatten() {
                        self.expr(p);
                    }
                    self.stmts(b);
                }
            }
            ExprKind::Block(b) => self.stmts(b),
            ExprKind::ArrayList(list) => {
                for x in list {
                    self.expr(x);
                }
            }
            ExprKind::Num(..) | ExprKind::Bool(_) => {}
        }
    }
}

/// Every qualified call's module part (`a.b.f(..)` -> `a.b`).
fn qualified_calls(b: &[Stmt], out: &mut HashSet<String>) {
    fn expr(e: &Expr, out: &mut HashSet<String>) {
        match &e.kind {
            ExprKind::Call(n, args) => {
                if let Some((m, _)) = n.rsplit_once('.') {
                    out.insert(m.to_string());
                }
                for a in args {
                    expr(a, out);
                }
            }
            ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(a, out),
            ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                expr(a, out);
                expr(b, out);
            }
            ExprKind::If(arms, else_) => {
                for (c, b) in arms {
                    expr(c, out);
                    qualified_calls(b, out);
                }
                if let Some(b) = else_ {
                    qualified_calls(b, out);
                }
            }
            ExprKind::Match(s, arms) => {
                expr(s, out);
                for (pats, b) in arms {
                    for p in pats.iter().flatten() {
                        expr(p, out);
                    }
                    qualified_calls(b, out);
                }
            }
            ExprKind::Block(b) => qualified_calls(b, out),
            ExprKind::ArrayList(l) => l.iter().for_each(|x| expr(x, out)),
            ExprKind::StructLit(_, f) => f.iter().for_each(|(_, x)| expr(x, out)),
            ExprKind::Num(..) | ExprKind::Bool(_) | ExprKind::Ident(_) => {}
        }
    }
    for s in b {
        match s {
            Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value), _) => expr(value, out),
            Stmt::Assign { target, value, .. } => {
                expr(target, out);
                expr(value, out);
            }
            Stmt::For { from, to, body, .. } => {
                expr(from, out);
                expr(to, out);
                qualified_calls(body, out);
            }
            Stmt::While { cond, body, .. } => {
                expr(cond, out);
                qualified_calls(body, out);
            }
            Stmt::Loop { body, .. } => qualified_calls(body, out),
            _ => {}
        }
    }
}

/// Keeps the items `roots` name and everything they reach (by name, through
/// calls, identifiers and struct literals); library items nobody reaches
/// are dropped before lowering, so an unused module costs nothing.
pub fn prune(items: Vec<Item>, roots: &HashSet<String>) -> Vec<Item> {
    let index: HashMap<String, usize> = items.iter().enumerate().map(|(k, i)| (i.name().to_string(), k)).collect();
    let mut keep = vec![false; items.len()];
    let mut todo: Vec<usize> = roots.iter().filter_map(|r| index.get(r).copied()).collect();
    while let Some(k) = todo.pop() {
        if keep[k] {
            continue;
        }
        keep[k] = true;
        let mut names = HashSet::new();
        item_names(&items[k], &mut names);
        for n in names {
            if let Some(&j) = index.get(&n) {
                if !keep[j] {
                    todo.push(j);
                }
            }
        }
    }
    items.into_iter().zip(keep).filter_map(|(i, k)| k.then_some(i)).collect()
}

fn item_names(it: &Item, out: &mut HashSet<String>) {
    fn expr(e: &Expr, out: &mut HashSet<String>) {
        match &e.kind {
            ExprKind::Ident(n) => {
                out.insert(n.clone());
            }
            ExprKind::Call(n, args) => {
                out.insert(n.clone());
                args.iter().for_each(|a| expr(a, out));
            }
            ExprKind::StructLit(n, f) => {
                out.insert(n.clone());
                f.iter().for_each(|(_, x)| expr(x, out));
            }
            ExprKind::Field(a, _) | ExprKind::Neg(a) | ExprKind::Not(a) => expr(a, out),
            ExprKind::Index(a, b) | ExprKind::Bin(_, a, b) | ExprKind::ArrayRepeat(a, b) => {
                expr(a, out);
                expr(b, out);
            }
            ExprKind::If(arms, else_) => {
                for (c, b) in arms {
                    expr(c, out);
                    stmts(b, out);
                }
                if let Some(b) = else_ {
                    stmts(b, out);
                }
            }
            ExprKind::Match(s, arms) => {
                expr(s, out);
                for (pats, b) in arms {
                    pats.iter().flatten().for_each(|p| expr(p, out));
                    stmts(b, out);
                }
            }
            ExprKind::Block(b) => stmts(b, out),
            ExprKind::ArrayList(l) => l.iter().for_each(|x| expr(x, out)),
            ExprKind::Num(..) | ExprKind::Bool(_) => {}
        }
    }
    fn stmts(b: &[Stmt], out: &mut HashSet<String>) {
        for s in b {
            match s {
                Stmt::Let { value, .. } | Stmt::Expr(value) | Stmt::Return(Some(value), _) => expr(value, out),
                Stmt::Assign { target, value, .. } => {
                    expr(target, out);
                    expr(value, out);
                }
                Stmt::For { from, to, body, .. } => {
                    expr(from, out);
                    expr(to, out);
                    stmts(body, out);
                }
                Stmt::While { cond, body, .. } => {
                    expr(cond, out);
                    stmts(body, out);
                }
                Stmt::Loop { body, .. } => stmts(body, out),
                _ => {}
            }
        }
    }
    match it {
        Item::Let { value, .. } | Item::Var { value, .. } => expr(value, out),
        Item::Struct { fields, .. } => fields.iter().for_each(|(_, e)| expr(e, out)),
        Item::Fn(f) => stmts(&f.body, out),
        Item::Use { .. } => {}
    }
}

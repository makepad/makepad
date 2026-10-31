//! `unused_imports` (rustc_resolve/src/check_unused.rs): `use` declarations nothing refers to.
//!
//! rustc records, while resolving names, which import each resolution went through. Relax
//! resolves imports to their targets without keeping the import a path went through, so the use
//! of an import is read from the names written in its scope instead: an import is used when its
//! name appears as a path (or single identifier) in the module or block it was declared in
//! (macro expansions count at their call site, other imports count through the first segment of
//! their path), when it names a trait whose methods a body calls, or when a path of the crate
//! reaches it as `self::` / `super::` / `crate::`. A name written where it could be shadowed
//! (a local variable, a generic parameter) still counts as a use: the lint never reports an
//! import rustc would call used.
//!
//! Imports exported from the crate (`pub use` reachable from the root) are never reported;
//! the message and the spans follow rustc's (`unused import: `a::b``, one span per unused leaf
//! of the use tree). Check-only; `makepad run` never calls this.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::tcx::Tcx;
use crate::lexer::T;
use std::collections::{HashMap, HashSet};

pub const UNUSED_IMPORTS: Lint = lint::lint("unused_imports");
pub const REDUNDANT_IMPORTS: Lint = lint::lint("redundant_imports");
pub const UNUSED_EXTERN_CRATES: Lint = lint::lint("unused_extern_crates");

/// Traits one of whose methods a checked body calls (collected with the bodies).
static METHOD_TRAITS: relax_session::sync::Mutex<Option<HashSet<DefId>>> = relax_session::sync::Mutex::new(None);

pub fn reset() {
    *METHOD_TRAITS.lock().unwrap_or_else(|e| e.into_inner()) = Some(HashSet::new());
}

/// A method or associated item a body resolved to: its trait counts as used.
pub fn note_method(prog: &Program, tcx: &Tcx, d: DefId) {
    let def = prog.def(d);
    if def.parent == NO_DEF { return; }
    let t = match prog.def(def.parent).kind {
        DefKind::Trait => def.parent,
        DefKind::Impl => match tcx.impl_trait.get(&def.parent) { Some(&t) => t, None => return },
        _ => return,
    };
    METHOD_TRAITS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(t);
}

/// An identifier token of a file and where it is written.
struct Ident {
    /// the call site for tokens of macro expansions
    site: u32,
    name: String,
    /// the segment before it when written after `::` (`m1::f`: `m1`)
    prefix: Option<String>,
    /// written as a macro: `name!(..)` or inside an attribute (`#[name]`, `#[derive(name)]`)
    mac: bool,
}

struct FileTokens {
    /// identifiers by site position (outside `use` items)
    idents: Vec<Ident>,
}

fn scan_tokens(prog: &Program, file: u32) -> FileTokens {
    let pf = &prog.files[file as usize];
    let mut idents = Vec::new();
    let mut kinds: Vec<(T, u32, u32)> = Vec::new();
    let mut i = 0u32;
    loop {
        let t = pf.tok(i);
        if t.kind == T::Eof { break; }
        kinds.push((t.kind, t.lo, t.hi));
        i += 1;
    }
    let text = |k: usize| String::from_utf8_lossy(pf.bytes(kinds[k].1, kinds[k].2)).into_owned();
    let mut in_use = false;
    // the token index a macro call's arguments end at: names there may be pasted into paths
    let mut mac_end = 0usize;
    // the token index an attribute's brackets end at
    let mut attr_end = 0usize;
    // inside `#[macro_use(..)]`: the names listed are no uses
    let mut in_macro_use = false;
    for k in 0..kinds.len() {
        if k >= mac_end && matches!(kinds[k].0, T::Ident | T::RawIdent) && k + 2 < kinds.len() && kinds[k + 1].0 == T::Not && matches!(kinds[k + 2].0, T::OpenParen | T::OpenBracket | T::OpenBrace) {
            let mut depth = 0i32;
            let mut e = k + 2;
            while e < kinds.len() {
                match kinds[e].0 { T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1, T::CloseParen | T::CloseBracket | T::CloseBrace => { depth -= 1; if depth == 0 { break; } } _ => {} }
                e += 1;
            }
            mac_end = e;
        }
        let (kind, lo, _) = kinds[k];
        if k >= attr_end && kind == T::Pound {
            let o = if k + 1 < kinds.len() && kinds[k + 1].0 == T::Not { k + 2 } else { k + 1 };
            if o < kinds.len() && kinds[o].0 == T::OpenBracket {
                let mut depth = 0i32;
                let mut e = o;
                while e < kinds.len() {
                    match kinds[e].0 { T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1, T::CloseParen | T::CloseBracket | T::CloseBrace => { depth -= 1; if depth == 0 { break; } } _ => {} }
                    e += 1;
                }
                attr_end = e;
                in_macro_use = o + 1 < kinds.len() && text(o + 1) == "macro_use";
            }
        }
        // the paths of a `use` item are no uses
        if in_use {
            if kind == T::Semi { in_use = false; }
            continue;
        }
        match kind {
            T::KwUse => in_use = true,
            T::Ident | T::RawIdent | T::KwSelfValue | T::KwSuper | T::KwCrate => {
                let prev = if k > 0 { Some(kinds[k - 1].0) } else { None };
                if prev == Some(T::Dot) && k >= mac_end { continue; }
                if in_macro_use && k < attr_end { continue; }
                let name = text(k);
                let name = name.strip_prefix("r#").map(str::to_string).unwrap_or(name);
                let prefix = if prev == Some(T::PathSep) && k >= 2 { Some(text(k - 2)) } else { None };
                let mac = k < attr_end || (k + 2 < kinds.len() && kinds[k + 1].0 == T::Not && matches!(kinds[k + 2].0, T::OpenParen | T::OpenBracket | T::OpenBrace));
                idents.push(Ident { site: pf.diagnostic_pos(lo), name, prefix, mac });
            }
            T::RawBody => {
                // the unparsed body of a macro: its words are names written at the call
                let site = pf.diagnostic_pos(lo);
                let bytes = pf.bytes(kinds[k].1, kinds[k].2).to_vec();
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                        let s = i;
                        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
                        let mut p = s;
                        while p > 0 && bytes[p - 1].is_ascii_whitespace() { p -= 1; }
                        // a method or field name is no path
                        if p > 0 && bytes[p - 1] == b'.' { continue; }
                        let prefix = if p >= 2 && &bytes[p - 2..p] == b"::" {
                            let mut e = p - 2;
                            while e > 0 && bytes[e - 1].is_ascii_whitespace() { e -= 1; }
                            let mut b = e;
                            while b > 0 && (bytes[b - 1].is_ascii_alphanumeric() || bytes[b - 1] == b'_') { b -= 1; }
                            Some(String::from_utf8_lossy(&bytes[b..e]).into_owned())
                        } else { None };
                        let mut a = i;
                        while a < bytes.len() && bytes[a].is_ascii_whitespace() { a += 1; }
                        let mac = a < bytes.len() && bytes[a] == b'!' && bytes.get(a + 1) != Some(&b'=');
                        idents.push(Ident { site, name: String::from_utf8_lossy(&bytes[s..i]).into_owned(), prefix, mac });
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
    }
    idents.sort_by_key(|i| i.site);
    FileTokens { idents }
}

/// A leaf of a use tree: what it imports and the source range of the tree node.
struct Leaf {
    /// the name it binds in the scope (None: `as _` or an unnameable glob)
    name: Option<String>,
    glob: bool,
    empty: bool,
    /// written `as _`
    under: bool,
    span: (u32, u32),
    /// the leaf's path as segments (for resolving the target)
    segs: Vec<PathSeg>,
}

fn leaves(prog: &Program, file: u32, ast: &Ast, t: &UseTree, start: u32, item_hi: u32, prefix: &mut Vec<PathSeg>, out: &mut Vec<Leaf>) {
    let pf = &prog.files[file as usize];
    match *t {
        UseTree::Name(seg, rename) => {
            let mut segs = prefix.clone();
            segs.push(seg);
            let end = rename.map_or(seg.name.hi, |r| r.hi);
            let name = match rename {
                Some(r) => { let n = pf.text(r); if n == "_" { None } else { Some(n.to_string()) } }
                None => {
                    if seg.kind() == SegKind::SelfValue { prefix.last().map(|s| pf.text(s.name).to_string()) } else { Some(pf.text(seg.name).to_string()) }
                }
            };
            let under = rename.is_some_and(|r| pf.text(r) == "_");
            out.push(Leaf { name, glob: false, empty: false, under, span: (start, end), segs });
        }
        UseTree::Path(seg, child) => {
            prefix.push(seg);
            let c = *ast.use_tree(child);
            leaves(prog, file, ast, &c, start, item_hi, prefix, out);
            prefix.pop();
        }
        UseTree::Glob => {
            // `*` has no segment of its own: ends at the last segment's end + `::*`
            let end = prefix.last().map_or(start, |s| s.name.hi);
            out.push(Leaf { name: None, glob: true, empty: false, under: false, span: (start, end + 3) , segs: prefix.clone() });
        }
        UseTree::Group(l) => {
            if ast.list(l).is_empty() {
                // `use a::{};` imports nothing: unused, as a whole
                out.push(Leaf { name: None, glob: false, empty: true, under: false, span: (start, item_hi) , segs: prefix.clone() });
            }
            for c in ast.list(l) {
                // a nested tree starts at its own first segment
                let s = first_pos(ast, c).unwrap_or(start);
                leaves(prog, file, ast, c, s, item_hi, prefix, out);
            }
        }
    }
}

fn ast_item_lo(prog: &Program, d: DefId) -> u32 {
    let def = prog.def(d);
    prog.files[def.file as usize].ast.item(def.item).lo
}

fn first_pos(ast: &Ast, t: &UseTree) -> Option<u32> {
    match *t {
        UseTree::Name(seg, _) | UseTree::Path(seg, _) => Some(seg.name.lo),
        UseTree::Group(l) => ast.list(l).first().and_then(|c| first_pos(ast, c)),
        UseTree::Glob => None,
    }
}

struct Scope<'a> {
    /// the module (or fn, for a block) whose attributes decide the lint level
    def: DefId,
    module: u32,
    /// a block of a body (its `self::` is the module around it)
    block: bool,
    file: u32,
    /// the byte ranges names in this scope are written in
    ranges: Vec<(u32, u32)>,
    items: Vec<ItemId>,
    ast: &'a Ast,
}

/// The macros the body of the `macro_rules!` called `name` invokes (`other!(..)`): the
/// invocation of a macro is a use of the names its expansion invokes in the caller's scope.
fn body_macro_calls(prog: &Program, name: &str, out: &mut Vec<String>) {
    use crate::macros::TNode;
    fn walk(ns: &[TNode], out: &mut Vec<String>) {
        let mut prev: Option<&str> = None;
        for n in ns {
            match n {
                TNode::Tok(t) => {
                    if t.kind == crate::lexer::T::Not { if let Some(p) = prev { out.push(p.to_string()); } }
                    prev = if matches!(t.kind, crate::lexer::T::Ident | crate::lexer::T::RawIdent) { Some(&t.text) } else { None };
                }
                TNode::Rep(inner, _, _) | TNode::Group(_, _, inner) => { walk(inner, out); prev = None; }
                _ => prev = None,
            }
        }
    }
    for d in prog.macros.defs.get(name).into_iter().flatten() {
        for (_, body) in &d.arms { walk(body, out); }
    }
}

/// `names` and every macro name their expansions invoke in turn.
fn macro_closure(prog: &Program, names: &mut HashSet<String>) {
    let mut todo: Vec<String> = names.iter().cloned().collect();
    while let Some(n) = todo.pop() {
        let mut calls = Vec::new();
        body_macro_calls(prog, &n, &mut calls);
        for c in calls { if names.insert(c.clone()) { todo.push(c); } }
    }
}

pub fn check(prog: &Program, tcx: &Tcx, own: &[bool], reach: &crate::lint_dead::Reach) {
    if !lint::enabled() { return; }
    let traits = METHOD_TRAITS.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let mut scopes: Vec<Scope> = Vec::new();
    for (m, md) in prog.mods.iter().enumerate() {
        if !own.get(md.krate as usize).copied().unwrap_or(false) { continue; }
        if md.lexical_parent.is_some() { continue; }
        let (file, items) = crate::lint_dead::module_items(prog, m as u32);
        let ast = &prog.files[file as usize].ast;
        let mut ranges = Vec::new();
        // each kept item's range reaches back to the end of the one before it: its attributes
        // (`#[derive(Widget)]`) and doc comments are part of the scope
        let mut prev_hi = if md.parent_mod.is_none() { 0 } else { ast_item_lo(prog, md.def) };
        for &it in &items {
            let item = ast.item(it);
            match item.kind {
                // child modules have scopes of their own; `use` items are checked, not scanned
                ItemKind::Mod(_) | ItemKind::Use(_) => {}
                // a `#[test]` fn is not part of the program outside a test build
                ItemKind::Fn(..) if lint::has_attr(prog, file, ast.list(item.attrs), "test") => {}
                _ => ranges.push((prev_hi.min(item.lo), item.hi)),
            }
            prev_hi = item.hi;
        }
        scopes.push(Scope { def: md.def, module: m as u32, block: false, file, ranges, items, ast });
    }
    // `use` declarations inside bodies: the block is the scope
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) { continue; }
        let ast = &prog.files[def.file as usize].ast;
        let Ok(Some(b)) = prog.fn_body(def.file, def.item) else { continue };
        collect_block_scopes(prog, DefId(i as u32), def.file, ast, b, &mut scopes);
    }
    // token sites are call-site positions; so are the ranges (code a macro wrote has its text in the
    // file's expansion area)
    for sc in scopes.iter_mut() {
        let pf = &prog.files[sc.file as usize];
        let mapped: Vec<(u32, u32)> = sc.ranges.iter().map(|&(lo, hi)| (pf.diagnostic_pos(lo), pf.diagnostic_pos(hi.saturating_sub(1)).saturating_add(1))).filter(|&(lo, hi)| hi > lo).collect();
        sc.ranges.extend(mapped);
    }
    let mut tokens: HashMap<u32, FileTokens> = HashMap::new();
    for sc in &scopes { tokens.entry(sc.file).or_insert_with(|| scan_tokens(prog, sc.file)); }
    // the macro names written in each scope (`name!(..)`, `#[name]`, `#[derive(name)]`): macro
    // bindings are not resolved through the module maps, so a macro import is judged by them
    let mut written: Vec<HashSet<String>> = Vec::new();
    for sc in &scopes {
        let toks = &tokens[&sc.file];
        let mut names: HashSet<String> = HashSet::new();
        for &(lo, hi) in &sc.ranges {
            let a = toks.idents.partition_point(|i| i.site < lo);
            for id in &toks.idents[a..] {
                if id.site >= hi { break; }
                if id.mac && id.prefix.is_none() { names.insert(id.name.clone()); }
            }
        }
        macro_closure(prog, &mut names);
        written.push(names);
    }
    for sc in &scopes {
        unused_extern_crates(prog, sc);
    }
    // the macro names written anywhere in each crate
    let mut crate_written: HashMap<u32, HashSet<String>> = HashMap::new();
    for (si, sc) in scopes.iter().enumerate() {
        if sc.module == u32::MAX { continue; }
        crate_written.entry(prog.mods[sc.module as usize].krate).or_default().extend(written[si].iter().cloned());
    }
    for sc in &scopes {
        unused_macro_use(prog, sc, &crate_written);
    }
    for (si, sc) in scopes.iter().enumerate() {
        let names = &written[si];
        for &it in &sc.items {
            let item = sc.ast.item(it);
            let ItemKind::Use(row) = item.kind else { continue };
            if lint::external(prog, sc.file, item.lo) { continue; }
            // a declaration a macro wrote: its scope's names cannot be located
            if item.lo >= prog.files[sc.file as usize].src_len { continue; }
            let u = *sc.ast.row(row);
            let mut ls = Vec::new();
            let start = first_pos(sc.ast, &u.tree).unwrap_or(item.lo);
            leaves(prog, sc.file, sc.ast, &u.tree, start, item.hi, &mut Vec::new(), &mut ls);
            let ctx = Ctx { prog, sc, names, traits: &traits };
            let mut unused: Vec<(u32, u32)> = Vec::new();
            // `pub use` that leaves the crate cannot be judged (a glob of nothing exports nothing)
            let exported = matches!(item.vis, Vis::Pub) && reach.defs.contains(&sc.def);
            // a glob imported again in the scope (a `pub` one overwrites a private one) is not judged
            let pf0 = &prog.files[sc.file as usize];
            let dup_glob = |l: &Leaf| -> bool {
                if !l.glob { return false; }
                let text: Vec<&str> = l.segs.iter().map(|s| pf0.text(s.name)).collect();
                let mut n = 0;
                for &o in &sc.items {
                    let oi = sc.ast.item(o);
                    let ItemKind::Use(r2) = oi.kind else { continue };
                    let u2 = *sc.ast.row(r2);
                    let mut l2 = Vec::new();
                    leaves(prog, sc.file, sc.ast, &u2.tree, first_pos(sc.ast, &u2.tree).unwrap_or(oi.lo), oi.hi, &mut Vec::new(), &mut l2);
                    n += l2.iter().filter(|x| x.glob && x.segs.iter().map(|s| pf0.text(s.name)).collect::<Vec<_>>() == text).count();
                }
                n > 1
            };
            for l in &ls {
                if dup_glob(l) { continue; }
                if exported && !(l.glob && ctx.glob_imports_nothing(l)) && !l.empty {
                    // check_import_as_underscore: an exported `as _` import of anything but a
                    // trait binds nothing a user can name
                    if l.under && !ctx.is_trait_import(l) { unused.push(l.span); }
                    continue;
                }
                if !ctx.leaf_used(l) {
                    unused.push(l.span);
                }
            }
            let _ = tcx;
            if !exported && !matches!(item.vis, Vis::Pub) {
                for l in &ls {
                    if !unused.contains(&l.span) && ctx.redundant(l) {
                        let pf = &prog.files[sc.file as usize];
                        let attrs = sc.ast.list(item.attrs).to_vec();
                        let name = l.name.as_deref().unwrap_or("");
                        let _ = pf;
                        lint::emit_in(prog, sc.def, &attrs, &REDUNDANT_IMPORTS, sc.file, l.span.0, &format!("the item `{}` is imported redundantly", name));
                    }
                }
            }
            if unused.is_empty() { continue; }
            let pf = &prog.files[sc.file as usize];
            let mut snippets: Vec<String> = unused.iter().map(|&(a, b)| format!("`{}`", String::from_utf8_lossy(pf.bytes(a, b)).trim_end_matches(|c: char| c == ';' || c.is_whitespace()))).collect();
            snippets.sort();
            let list = match snippets.len() {
                1 => snippets[0].clone(),
                2 => format!("{} and {}", snippets[0], snippets[1]),
                n => format!("{}, and {}", snippets[..n - 1].join(", "), snippets[n - 1]),
            };
            let msg = format!("unused import{}: {}", if snippets.len() == 1 { "" } else { "s" }, list);
            let attrs = sc.ast.list(item.attrs).to_vec();
            lint::emit_in(prog, sc.def, &attrs, &UNUSED_IMPORTS, sc.file, unused[0].0, &msg);
        }
    }
}

/// `unused_extern_crates` (check_unused.rs report_unused_extern_crate_items, the fully unused
/// case): an `extern crate` item nothing resolved through, neither its module binding nor (a
/// renamed one at the crate root) its extern prelude entry.
fn unused_extern_crates(prog: &Program, sc: &Scope<'_>) {
    if sc.block || sc.module == u32::MAX { return; }
    let krate = prog.mods[sc.module as usize].krate;
    let ast = sc.ast;
    for &it in &sc.items {
        let item = ast.item(it);
        let ItemKind::ExternCrate(rename) = item.kind else { continue };
        if matches!(item.vis, Vis::Pub) || lint::external(prog, sc.file, item.lo) || item.lo >= prog.files[sc.file as usize].src_len { continue; }
        // attributes (`#[macro_use]`) may have semantics a removal loses
        if !ast.list(item.attrs).is_empty() { continue; }
        let pf = &prog.files[sc.file as usize];
        let shown = pf.text(item.name).to_string();
        if shown == "self" { continue; }
        let bind_text = rename.map_or(shown.clone(), |r| pf.text(r).to_string());
        if bind_text.starts_with('_') { continue; }
        let bind = prog.syms.intern_late(&bind_text);
        let root = prog.crates[krate as usize].root_mod == sc.module;
        let used = prog.import_binding_used(sc.module, bind) || (rename.is_some() && root && prog.extern_prelude_used(krate, bind));
        if used { continue; }
        let attrs = ast.list(item.attrs).to_vec();
        lint::emit_in(prog, sc.def, &attrs, &UNUSED_EXTERN_CRATES, sc.file, item.lo, "unused extern crate");
    }
}

pub const UNUSED_MACRO_USE_MSG: &str = "unused `#[macro_use]` import";

/// `#[macro_use] extern crate dep;` (also `#[macro_use(a, b)]`): unused when none of the macros it
/// brings into the macro_use prelude is invoked by name in the crate (an approximation of
/// rustc's per-resolution record: a macro reached only by path counts as a use here too).
fn unused_macro_use(prog: &Program, sc: &Scope<'_>, written: &HashMap<u32, HashSet<String>>) {
    if sc.block || sc.module == u32::MAX { return; }
    let krate = prog.mods[sc.module as usize].krate;
    let Some(used) = written.get(&krate) else { return };
    let ast = sc.ast;
    let pf = &prog.files[sc.file as usize];
    for &it in &sc.items {
        let item = ast.item(it);
        let ItemKind::ExternCrate(_) = item.kind else { continue };
                if lint::external(prog, sc.file, item.lo) || item.lo >= pf.src_len { continue; }
        let target = pf.text(item.name).to_string();
        for a in ast.list(item.attrs) {
            if lint::attr_path(prog, sc.file, a) != b"macro_use" { continue; }
            let p = prog.src(sc.file);
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            let attrs = ast.list(item.attrs).to_vec();
            // `macro_use(a, b)`: the listed names
            let mut names: Vec<(String, u32)> = Vec::new();
            if hi > lo + 1 && crate::cfg::tok_str(p, lo + 1) == b"(" {
                for i in lo + 2..hi {
                    let t = crate::cfg::tok_str(p, i);
                    if t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') { names.push((String::from_utf8_lossy(t).into_owned(), p.tok(i).lo)); }
                }
            }
            if names.is_empty() {
                // every exported macro of the crate
                let mut all: Vec<String> = prog.macros.defs.iter().filter(|(_, v)| v.iter().any(|d| d.exported && d.krate == target)).map(|(k, _)| k.clone()).collect();
                if matches!(target.as_str(), "core" | "std" | "alloc") { all.extend(crate::program::BUILTIN_MACROS.iter().map(|s| s.to_string())); }
                if !all.iter().any(|n| used.contains(n)) {
                    lint::emit_in(prog, sc.def, &attrs, &UNUSED_IMPORTS, sc.file, p.tok(lo).lo, UNUSED_MACRO_USE_MSG);
                }
            } else {
                for (n, pos) in names {
                    if !used.contains(&n) { lint::emit_in(prog, sc.def, &attrs, &UNUSED_IMPORTS, sc.file, pos, UNUSED_MACRO_USE_MSG); }
                }
            }
        }
    }
}

fn collect_block_scopes<'a>(prog: &Program, owner: DefId, file: u32, ast: &'a Ast, b: BlockId, out: &mut Vec<Scope<'a>>) {
    use crate::ast_visit::{self, Visitor};
    struct W<'a, 'p> { prog: &'p Program, owner: DefId, file: u32, ast: &'a Ast, out: &'p mut Vec<Scope<'a>> }
    impl<'a, 'p> Visitor<'a> for W<'a, 'p> {
        fn ast(&self) -> &'a Ast { self.ast }
        fn block(&mut self, b: BlockId) {
            let ast = self.ast;
            let items: Vec<ItemId> = ast.stmts(b).iter().filter_map(|s| if let Stmt::Item(i) = s { Some(*i) } else { None }).collect();
            if items.iter().any(|&i| matches!(ast.item(i).kind, ItemKind::Use(_))) {
                let blk = ast.block(b);
                let module = self.prog.block_mods.get(&(self.file, b.0)).copied().unwrap_or(u32::MAX);
                // the block minus the `use` items
                let mut ranges = vec![(blk.lo, blk.hi)];
                for &i in &items {
                    if matches!(ast.item(i).kind, ItemKind::Use(_)) {
                        let (lo, hi) = (ast.item(i).lo, ast.item(i).hi);
                        let mut next = Vec::new();
                        for (a, z) in ranges {
                            if lo >= a && hi <= z { next.push((a, lo)); next.push((hi, z)); } else { next.push((a, z)); }
                        }
                        ranges = next;
                    }
                }
                self.out.push(Scope { def: self.owner, module, block: true, file: self.file, ranges, items, ast });
            }
            ast_visit::walk_block(self, b);
        }
    }
    W { prog, owner, file, ast, out }.block(b);
}

struct Ctx<'a, 'b> {
    prog: &'a Program,
    sc: &'b Scope<'a>,
    /// the macro names written in the scope
    names: &'b HashSet<String>,
    traits: &'b HashSet<DefId>,
}

impl Ctx<'_, '_> {
    /// A glob whose target has no name the importing module may use.
    fn glob_imports_nothing(&self, l: &Leaf) -> bool {
        let prog = self.prog;
        let pf = &prog.files[self.sc.file as usize];
        if self.sc.block { return false; }
        let segs: Vec<(SegKind, crate::program::Sym)> = l.segs.iter().map(|s| (s.kind(), prog.syms.intern_late(pf.text(s.name)))).collect();
        let Some(target) = prog.resolve_mod_path(self.sc.module, false, &segs, false, true).or_else(|| prog.resolve_mod_path(self.sc.module, true, &segs, false, true)) else { return false };
        let td = prog.def(target);
        if td.kind != DefKind::Mod { return false; }
        let m = &prog.mods[td.sub as usize];
        let visible = |d: DefId| prog.def(d).is_pub;
        !(m.types.values().any(|&d| visible(d)) || m.values.values().any(|&d| visible(d)) || m.macros.values().any(|&d| visible(d)) || m.globs.iter().any(|_| true))
    }

    /// Whether the leaf's path resolves to a trait in the type namespace.
    /// (An unresolvable path counts as a trait: it is never reported.)
    fn is_trait_import(&self, l: &Leaf) -> bool {
        let prog = self.prog;
        let pf = &prog.files[self.sc.file as usize];
        if self.sc.block || self.sc.module == u32::MAX { return true; }
        let Some((last, prefix)) = l.segs.split_last() else { return true };
        let segs: Vec<(SegKind, crate::program::Sym)> = prefix.iter().map(|s| (s.kind(), prog.syms.intern_late(pf.text(s.name)))).collect();
        let name = prog.syms.intern_late(pf.text(last.name));
        let found = if segs.is_empty() {
            prog.lookup_in_scope(self.sc.module, name, true)
        } else {
            let Some(m) = prog.resolve_mod_path(self.sc.module, false, &segs, false, true).or_else(|| prog.resolve_mod_path(self.sc.module, true, &segs, false, true)) else { return true };
            let td = prog.def(m);
            if td.kind != DefKind::Mod { return true; }
            prog.mods[td.sub as usize].types.get(&name).copied()
        };
        match found {
            Some(d) => prog.def(d).kind == DefKind::Trait,
            None => true,
        }
    }

    /// check_for_redundant_imports: a single import `use a::name;` (no rename) of an item the
    /// scope reaches without it (outside the module: the preludes; in a block: the enclosing
    /// scopes) under the same name, in every namespace the import binds.
    fn redundant(&self, l: &Leaf) -> bool {
        let prog = self.prog;
        let sc = self.sc;
        if l.glob || l.empty || l.under || sc.module == u32::MAX { return false; }
        let Some(name) = l.name.as_deref() else { return false };
        let pf = &prog.files[sc.file as usize];
        // `use source as target` with another name is never redundant
        if l.segs.last().is_none_or(|s| pf.text(s.name) != name) { return false; }
        let sym = prog.syms.intern_late(name);
        let mut any = false;
        for ns in [true, false] {
            let Some(mine) = prog.lookup_in_module(sc.module, sym, ns, &mut Vec::new()) else { continue };
            let other = if sc.block {
                match prog.mods[sc.module as usize].lexical_parent { Some(p) => prog.lookup_in_scope(p, sym, ns), None => None }
            } else {
                prog.lookup_in_scope_outside(sc.module, sym, ns)
            };
            match other {
                Some(o) if o == mine => any = true,
                _ => return false,
            }
        }
        any
    }

    fn leaf_used(&self, l: &Leaf) -> bool {
        let prog = self.prog;
        let sc = self.sc;
        let pf = &prog.files[sc.file as usize];
        if sc.module == u32::MAX { return !l.empty; }
        if l.glob {
            let segs: Vec<(SegKind, crate::program::Sym)> = l.segs.iter().map(|s| (s.kind(), prog.syms.intern_late(pf.text(s.name)))).collect();
            let Some(target) = prog.resolve_mod_path(sc.module, false, &segs, false, true).or_else(|| prog.resolve_mod_path(sc.module, true, &segs, false, true)) else { return true };
            let td = prog.def(target);
            // a proc-macro crate's macros are not bindings Relax lists: its glob cannot be judged
            if prog.crates.get(td.krate as usize).is_some_and(|c| c.proc_macro) { return true; }
            if prog.glob_import_used(sc.module, target) { return true; }
            if td.kind == DefKind::Mod {
                // the module and what it glob-imports in turn: its macros written here, its
                // traits whose methods are called
                let mut stack = vec![td.sub];
                let mut seen: HashSet<u32> = HashSet::new();
                while let Some(mi) = stack.pop() {
                    if !seen.insert(mi) { continue; }
                    let m = &prog.mods[mi as usize];
                    if m.macros.keys().any(|k| self.names.contains(prog.syms.str(*k))) { return true; }
                    for (_, &d) in m.types.iter() {
                        if prog.def(d).kind == DefKind::Trait && self.traits.contains(&d) { return true; }
                    }
                    for (d, _) in &m.anon_traits { if self.traits.contains(d) { return true; } }
                    for &(g, _) in &m.globs { if prog.def(g).kind == DefKind::Mod { stack.push(prog.def(g).sub); } }
                }
            }
            return false;
        }
        if l.empty { return false; }
        let Some(name) = l.name.as_deref() else { return true };
        if prog.import_binding_used(sc.module, prog.syms.intern_late(name)) || self.names.contains(name) { return true; }
        // a trait is used by calling its methods
        for ns in [true, false] {
            if let Some(d) = prog.lookup_in_scope(sc.module, prog.syms.intern_late(name), ns) {
                if prog.def(d).kind == DefKind::Trait && self.traits.contains(&d) { return true; }
            }
        }
        false
    }
}

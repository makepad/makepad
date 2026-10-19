//! Lints of rustc_lint / rustc_resolve that need more than the syntax of one node: they use the
//! lifetime resolution (`mismatched_lifetime_syntaxes`, `elided_lifetimes_in_paths`), the
//! macro definitions, or the source text between tokens. Check-only.

use crate::ast::*;
use crate::lint::{self, Level, Lint};
use crate::lint_early::W;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::lifetime_resolve::{LifetimeRes, LifetimeResolution};
use crate::tcx::Tcx;
use std::sync::Arc;

pub const MISMATCHED_LIFETIME_SYNTAXES: Lint = lint::lint("mismatched_lifetime_syntaxes", Level::Warn, &[]);
pub const ELIDED_LIFETIMES_IN_PATHS: Lint = lint::lint("elided_lifetimes_in_paths", Level::Allow, &["rust_2018_idioms"]);

/// The lifetime resolution of a definition's signature and body.
pub(crate) fn names(prog: &Program, tcx: &Tcx, d: DefId) -> Arc<LifetimeResolution> { tcx.resolve_lifetime_names(prog, d) }

/// Reports `lint` at the token at `pos`, where rustc puts the lint: a token of a `macro_rules!`
/// body defined in the same file at its place in the definition.
pub(crate) fn emit(prog: &Program, d: DefId, lint: &Lint, file: u32, pos: u32, msg: &str) {
    emit_in(prog, d, &[], lint, file, pos, msg)
}

pub(crate) fn emit_in(prog: &Program, d: DefId, inner: &[Attr], lint: &Lint, file: u32, pos: u32, msg: &str) {
    lint::emit_in(prog, d, inner, lint, file, prog.files[file as usize].lint_pos(pos), msg)
}

// ---------------------------------------------------------------- mismatched_lifetime_syntaxes

#[derive(Clone, Copy, PartialEq, Eq)]
enum Syntax { Implicit, Anonymous, Bound }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source { Reference, Path, Outlives, Precise }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cat { Hidden, Elided, Named }

#[derive(Clone, Copy)]
struct Info { res: LifetimeRes, cat: Cat, pos: u32 }

struct Collector<'a, 'b> {
    w: &'a W<'b>,
    names: &'a LifetimeResolution,
    out: Vec<Info>,
    /// argument position: `impl Trait` is an anonymous type parameter there, its bounds are not
    /// part of the type
    input: bool,
}

impl Collector<'_, '_> {
    fn push(&mut self, res: Option<LifetimeRes>, syntax: Syntax, source: Source, pos: u32) {
        let Some(res) = res else { return };
        // lifetimes that failed to resolve are reported elsewhere
        if matches!(res, LifetimeRes::Error | LifetimeRes::Infer) { return; }
        let cat = match (syntax, source) {
            (Syntax::Implicit, Source::Reference) | (Syntax::Anonymous, _) => Cat::Elided,
            (Syntax::Implicit, Source::Path) => Cat::Hidden,
            (Syntax::Bound, _) => Cat::Named,
            // `+ '_` is elided, `+ 'a` named; an implicit one cannot be written there
            (Syntax::Implicit, _) => return,
        };
        self.out.push(Info { res, cat, pos });
    }

    fn written(&mut self, lt: Ident, source: Source) {
        let syntax = if self.w.pf.text(lt) == "'_" { Syntax::Anonymous } else { Syntax::Bound };
        let res = self.names.named(lt.lo, lt.hi);
        self.push(res, syntax, source, lt.lo);
    }

    fn ty(&mut self, t: TyId) {
        let ast = self.w.ast;
        match *ast.ty(t) {
            Ty::Ref(lt, _, inner) => {
                match lt {
                    Some(l) => self.written(l, Source::Reference),
                    None => {
                        let pos = self.w.ty_start(t).unwrap_or(self.w.cur);
                        self.push(self.names.reference(t.0), Syntax::Implicit, Source::Reference, pos);
                    }
                }
                self.ty(inner);
            }
            Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.ty(x),
            Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x); },
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                for &x in ast.list(f.params) { self.ty(x); }
                if let Some(r) = f.ret { self.ty(r); }
            }
            Ty::Path(p) => self.path(&p, t),
            Ty::ImplTrait(b) => if !self.input { self.bounds(b, t) },
            Ty::DynTrait(b, _) => self.bounds(b, t),
            Ty::Never | Ty::Infer | Ty::Mac(_) | Ty::ImplicitSelf | Ty::CVarArgs => {}
        }
    }

    fn bounds(&mut self, l: List<Bound>, owner: TyId) {
        let ast = self.w.ast;
        // the trait bounds first, the lifetime bound after them (intravisit TraitObject)
        for b in ast.list(l) {
            if let Bound::Trait { path, .. } = *b { self.path(&path, owner); }
        }
        for b in ast.list(l) {
            match *b {
                Bound::Lifetime(lt) => self.written(lt, Source::Outlives),
                Bound::Use(args) => {
                    if let Some(GenericArgs::Angle(l)) = ast.gargs(args) {
                        for a in ast.list(*l) { if let GenericArg::Lifetime(lt) = *a { self.written(lt, Source::Precise); } }
                    }
                }
                Bound::Trait { .. } => {}
            }
        }
    }

    fn path(&mut self, p: &Path, owner: TyId) {
        let ast = self.w.ast;
        if p.has_qself() {
            if let Some(q) = ast.qself(p) {
                self.ty(q.ty);
                if let Some(tp) = q.trait_path { self.path(&tp, owner); }
            }
        }
        for (i, seg) in ast.segs(p.segs).iter().enumerate() {
            // the hidden lifetimes come first in the lowered arguments
            let mut k = 0;
            while let Some(res) = self.names.path(p.segs.lo + i as u32, k) {
                let pos = self.w.ty_start(owner).unwrap_or(seg.name.lo);
                self.push(Some(res), Syntax::Implicit, Source::Path, pos);
                k += 1;
            }
            match ast.gargs(seg.args()) {
                Some(GenericArgs::Angle(l)) => {
                    for a in ast.list(*l) { if let GenericArg::Lifetime(lt) = *a { self.written(lt, Source::Path); } }
                    for a in ast.list(*l) { if let GenericArg::Type(t) = *a { self.ty(t); } }
                    for a in ast.list(*l) {
                        match *a {
                            GenericArg::Binding(_, _, t) => self.ty(t),
                            GenericArg::Constraint(_, _, b) => self.bounds(b, owner),
                            _ => {}
                        }
                    }
                }
                Some(GenericArgs::Paren(tys, ret)) => {
                    for &t in ast.list(*tys) { self.ty(t); }
                    if let Some(r) = ret { self.ty(*r); }
                }
                None => {}
            }
        }
    }
}

/// The position of the first token of a fn's parameter list (a `&self` receiver is the first
/// parameter).
fn params_start(w: &W, item: &Item) -> u32 {
    let pf = w.pf;
    let mut at = item.name.hi;
    let mut depth = 0i32;
    let n = pf.src_len;
    // generics: skip to the matching `>`
    loop {
        let b = pf.bytes(at.min(n), (at + 1).min(n));
        if b.is_empty() { return item.lo; }
        match b[0] {
            b'<' => depth += 1,
            b'>' => { depth -= 1; if depth <= 0 { at += 1; break; } }
            b'-' if pf.bytes(at + 1, (at + 2).min(n)) == b">" => at += 1,
            b'(' if depth == 0 => break,
            _ => {}
        }
        at += 1;
    }
    loop {
        let b = pf.bytes(at.min(n), (at + 1).min(n));
        if b.is_empty() { return item.lo; }
        if b[0] == b'(' { at += 1; break; }
        at += 1;
    }
    while let [c, ..] = pf.bytes(at.min(n), (at + 1).min(n)) {
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') { at += 1; } else { break; }
    }
    at
}

/// rustc_lint `LifetimeSyntax::check_fn_like`.
fn mismatched_lifetime_syntaxes(prog: &Program, tcx: &Tcx, d: DefId) {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let item = *ast.item(def.item);
    let ItemKind::Fn(sig, _, _) = item.kind else { return };
    let sig = *ast.fn_sig(sig);
    // no output, or no input: nothing can disagree
    let Some(ret) = sig.ret else { return };
    if sig.self_param.is_none_or(|s| matches!(s, SelfParam::Value(_))) && ast.list(sig.params).is_empty() { return; }
    let names = names(prog, tcx, d);
    let mut w = W::new(prog, def.file, d);
    w.cur = item.lo;
    let mut outputs = Collector { w: &w, names: &names, out: Vec::new(), input: false };
    outputs.ty(ret);
    let outputs = outputs.out;
    if outputs.is_empty() { return; }
    let mut inputs = Collector { w: &w, names: &names, out: Vec::new(), input: true };
    match sig.self_param {
        Some(SelfParam::Ref(lt, _)) => match lt {
            Some(l) => inputs.written(l, Source::Reference),
            None => {
                let pos = params_start(&w, &item);
                inputs.push(names.self_reference(), Syntax::Implicit, Source::Reference, pos);
            }
        },
        Some(SelfParam::Typed(_, t)) => inputs.ty(t),
        _ => {}
    }
    for p in ast.list(sig.params) { inputs.ty(p.ty); }
    let inputs = inputs.out;
    // the groups, in the order of the output's lifetimes
    let mut keys: Vec<LifetimeRes> = Vec::new();
    for o in &outputs { if !keys.contains(&o.res) { keys.push(o.res); } }
    for key in keys {
        let ins: Vec<&Info> = inputs.iter().filter(|i| i.res == key).collect();
        let outs: Vec<&Info> = outputs.iter().filter(|i| i.res == key).collect();
        let Some(first) = ins.first() else { continue };
        if ins.iter().chain(outs.iter()).all(|i| i.cat == first.cat) { continue; }
        let count = |c: Cat| ins.iter().chain(outs.iter()).filter(|i| i.cat == c).count();
        let (hidden, elided, named) = (count(Cat::Hidden), count(Cat::Elided), count(Cat::Named));
        let msg = if named == 0 { "hiding a lifetime that's elided elsewhere is confusing" }
            else if elided == 0 { "hiding a lifetime that's named elsewhere is confusing" }
            else if hidden == 0 { "eliding a lifetime that's named elsewhere is confusing" }
            else { "hiding or eliding a lifetime that's named elsewhere is confusing" };
        // the primary spans: the hidden then the elided ones of the inputs, then of the outputs
        let pos = [&ins, &outs].iter().find_map(|side| {
            [Cat::Hidden, Cat::Elided].iter().find_map(|c| side.iter().find(|i| i.cat == *c).map(|i| i.pos))
        }).unwrap_or(item.lo);
        if lint::external(prog, def.file, pos) { continue; }
        emit(prog, d, &MISMATCHED_LIFETIME_SYNTAXES, def.file, pos, msg);
    }
}

// ---------------------------------------------------------------- entry points

/// The definition-level passes of this file over one definition.
pub fn check_def_item(prog: &Program, tcx: &Tcx, d: DefId) {
    match prog.def(d).kind {
        DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => mismatched_lifetime_syntaxes(prog, tcx, d),
        DefKind::Struct | DefKind::Union | DefKind::Enum => explicit_outlives_requirements(prog, d),
        _ => {}
    }
}

// ---------------------------------------------------------------- attribute meta items

/// One item of an attribute's argument list (rustc_attr_parsing `MetaItemOrLitParser`).
#[derive(Debug)]
enum Meta {
    Lit { pos: u32, tok: Vec<u8> },
    Item { pos: u32, path: Vec<Vec<u8>>, args: Args },
}

#[derive(Debug)]
enum Args {
    None,
    List { items: Vec<Meta> },
    /// `= value`: the value's first token
    Value { pos: u32, tok: Vec<u8> },
}

fn is_literal_token(t: &[u8]) -> bool {
    match t.first() {
        Some(b'"') | Some(b'\'') | Some(b'0'..=b'9') => true,
        Some(b'b') | Some(b'r') | Some(b'c') => matches!(t.get(1), Some(b'"') | Some(b'\'') | Some(b'#')),
        _ => t == b"true" || t == b"false",
    }
}

/// Tokens `lo..hi` split into meta items at top-level commas.
fn parse_meta_items(pf: &crate::parser::ParsedFile, lo: usize, hi: usize) -> Vec<Meta> {
    let tok = |i: usize| pf.tok(i as u32);
    let text = |i: usize| { let t = tok(i); pf.bytes(t.lo, t.hi).to_vec() };
    let mut out = Vec::new();
    let mut i = lo;
    while i < hi {
        let first = text(i);
        let pos = tok(i).lo;
        if is_literal_token(&first) {
            out.push(Meta::Lit { pos, tok: first });
            i += 1;
        } else {
            let mut path = Vec::new();
            // `a::b::c` (a leading `::` is kept as an empty segment)
            loop {
                if i < hi && text(i) == b"::" { i += 1; continue; }
                if i < hi && !matches!(text(i).as_slice(), b"," | b"(" | b"=" | b")") { path.push(text(i)); i += 1; } else { break; }
                if i < hi && text(i) == b"::" { i += 1; } else { break; }
            }
            let args = if i < hi && text(i) == b"(" {
                let open = i;
                let mut depth = 0i32;
                while i < hi {
                    match text(i).as_slice() { b"(" | b"[" | b"{" => depth += 1, b")" | b"]" | b"}" => { depth -= 1; if depth == 0 { break; } } _ => {} }
                    i += 1;
                }
                let items = parse_meta_items(pf, open + 1, i.min(hi));
                i += 1;
                Args::List { items }
            } else if i < hi && text(i) == b"=" {
                let vpos = if i + 1 < hi { tok(i + 1).lo } else { tok(i).lo };
                let v = if i + 1 < hi { text(i + 1) } else { Vec::new() };
                i += 2;
                // an expression value spans to the next comma
                let mut depth = 0i32;
                while i < hi {
                    match text(i).as_slice() { b"(" | b"[" | b"{" => depth += 1, b")" | b"]" | b"}" => depth -= 1, b"," if depth == 0 => break, _ => {} }
                    i += 1;
                }
                Args::Value { pos: vpos, tok: v }
            } else { Args::None };
            out.push(Meta::Item { pos, path, args });
        }
        // skip to after the next top-level comma
        let mut depth = 0i32;
        while i < hi {
            match text(i).as_slice() { b"(" | b"[" | b"{" => depth += 1, b")" | b"]" | b"}" => depth -= 1, b"," if depth == 0 => { i += 1; break; } _ => {} }
            i += 1;
        }
    }
    out
}

/// A string literal token's value; None when the token is no string.
fn string_value(t: &[u8]) -> Option<String> {
    let (body, raw) = match t {
        [b'"', rest @ .., b'"'] => (rest, false),
        [b'r', rest @ ..] => {
            let hashes = rest.iter().take_while(|&&c| c == b'#').count();
            let inner = &rest[hashes..];
            if inner.len() < 2 + hashes || inner[0] != b'"' { return None; }
            (&inner[1..inner.len() - 1 - hashes], true)
        }
        _ => return None,
    };
    let s = String::from_utf8_lossy(body).into_owned();
    if raw { return Some(s); }
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' { out.push(c); continue; }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                if let Some(c) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) { out.push(c); }
            }
            Some('u') => {
                let mut h = String::new();
                if it.peek() == Some(&'{') { it.next(); while let Some(c) = it.next() { if c == '}' { break; } if c != '_' { h.push(c); } } }
                if let Some(c) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) { out.push(c); }
            }
            Some('\n') => { while it.peek().is_some_and(|c| c.is_whitespace()) { it.next(); } }
            Some(c) => out.push(c),
            None => {}
        }
    }
    Some(out)
}

pub const INVALID_DOC_ATTRIBUTES: Lint = lint::lint("invalid_doc_attributes", Level::Warn, &[]);
pub const UNKNOWN_DIAGNOSTIC_ATTRIBUTES: Lint = lint::lint("unknown_diagnostic_attributes", Level::Warn, &["unknown_or_malformed_diagnostic_attributes"]);
pub const UNUSED_ATTRIBUTES: Lint = crate::lint_early::UNUSED_ATTRIBUTES;
/// A malformed attribute that rustc rejects outright (no lint of its own): never allowed.
pub const MALFORMED_ATTRIBUTE: Lint = lint::lint("malformed_attribute", Level::Forbid, &[]);

/// Whether `#![feature(name)]` is written in the crate root of `krate`.
fn feature_enabled(prog: &Program, krate: u32, name: &str) -> bool {
    let rm = prog.crates[krate as usize].root_mod;
    let file = prog.mods[rm as usize].file;
    prog.files[file as usize].ast.root_attrs.iter().any(|a| {
        lint::attr_path(prog, file, a) == b"feature" && lint::attr_args(prog, file, a).iter().any(|w| w == name)
    })
}

/// The attributes of one node, outer then inner, each with the file it was written in.
pub(crate) struct AttrNode<'a> {
    pub d: DefId,
    /// attributes of enclosing nodes between `d` and this node (for items without a definition: its own)
    pub inner: &'a [Attr],
    pub attrs: &'a [(u32, Attr)],
    /// the crate root's attributes
    pub is_crate: bool,
    /// an `extern crate` / module / crate (the targets of `#[macro_use]`)
    pub macro_use_target: bool,
}

const DIAGNOSTIC_ATTRIBUTES: &[(&str, Option<&str>)] = &[
    ("on_unimplemented", None), ("do_not_recommend", None), ("on_move", Some("diagnostic_on_move")),
    ("on_const", Some("diagnostic_on_const")), ("on_unknown", Some("diagnostic_on_unknown")),
    ("on_unmatch_args", Some("diagnostic_on_unmatch_args")),
];

/// `OnDuplicate::Warn` / `WarnButFutureError` of the single-attribute parsers of
/// rustc_attr_parsing: the second use of such an attribute on one node is unused.
const WARN_ON_DUPLICATE: &[&str] = &["cold", "export_name", "track_caller", "no_mangle", "crate_name", "recursion_limit",
    "type_length_limit", "no_std", "no_main", "windows_subsystem", "no_builtins", "diagnostic::do_not_recommend", "inline",
    "link_name", "link_section", "automatically_derived", "macro_escape", "macro_export", "must_use", "no_implicit_prelude",
    "no_link", "non_exhaustive", "path", "ignore", "should_panic"];

const CRATE_LEVEL_DOC: &[&str] = &["html_favicon_url", "html_logo_url", "html_no_source", "html_playground_url", "html_root_url",
    "issue_tracker_base_url", "rust_logo"];
const NOT_CRATE_LEVEL_DOC: &[&str] = &["alias", "keyword", "attribute", "fake_variadic", "search_unbox"];
const DOC_NO_ARGS: &[&str] = &["hidden", "masked", "notable_trait", "inline", "no_inline", "html_no_source", "fake_variadic", "search_unbox", "rust_logo"];

fn joined(path: &[Vec<u8>]) -> String {
    path.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>().join("::")
}

/// Checks of rustc_attr_parsing / rustc_passes check_attr that read an attribute's arguments:
/// the `diagnostic` namespace, `doc` attributes, empty lint attributes, duplicate attributes,
/// `#[macro_use]` lists.
pub(crate) fn attr_checks(prog: &Program, n: &AttrNode) {
    let d = n.d;
    let krate = prog.def(d).krate;
    // duplicates: the first use of each attribute name is the one in force
    let mut seen: Vec<(Vec<u8>, u32)> = Vec::new();
    // `#[macro_use]` state: Some(first position) once all macros are used; the specific lists seen so far
    let mut use_all: Option<u32> = None;
    let mut specific: Vec<u32> = Vec::new();
    for &(file, a) in n.attrs {
        let pf = &prog.files[file as usize];
        let apos = crate::lint_early::attr_pos(pf, &a);
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        let metas = parse_meta_items(pf, lo, hi);
        let Some(Meta::Item { path, args, .. }) = metas.first() else { continue };
        let emit = |lint: &Lint, pos: u32, msg: &str| {
            if !lint::external(prog, file, pos) { emit_in(prog, d, n.inner, lint, file, pos, msg); }
        };
        let word = if path.len() == 1 { Some(String::from_utf8_lossy(&path[0]).into_owned()) } else { None };
        let full = joined(path);
        // ---- the diagnostic namespace
        if path.len() >= 2 && path[0] == b"diagnostic" {
            let name = String::from_utf8_lossy(&path[1]).into_owned();
            if !DIAGNOSTIC_ATTRIBUTES.iter().any(|(k, f)| *k == name && f.is_none_or(|f| feature_enabled(prog, krate, f))) {
                // the position of the name after `diagnostic::`
                let mut i = lo;
                while i < hi && pf.bytes(pf.tok(i as u32).lo, pf.tok(i as u32).hi) != b"::" { i += 1; }
                emit(&UNKNOWN_DIAGNOSTIC_ATTRIBUTES, pf.tok((i + 1).min(hi - 1) as u32).lo, "unknown diagnostic attribute");
            }
        }
        // ---- lint attributes: empty lists, `reason` only, crate-level-only lints
        if matches!(word.as_deref(), Some("allow" | "warn" | "deny" | "forbid" | "expect")) {
            let name = word.clone().unwrap();
            if let Args::List { items, .. } = args {
                let note = if items.is_empty() { Some(format!("attribute `{}` with an empty list has no effect", name)) }
                    else if let [Meta::Item { path, args: Args::Value { .. }, .. }] = &items[..] {
                        if path.len() == 1 && path[0] == b"reason" { Some(format!("attribute `{}` without any lints has no effect", name)) } else { None }
                    } else { None };
                if note.is_some() { emit(&UNUSED_ATTRIBUTES, apos, "unused attribute"); }
                else {
                    let names = |ls: &[&str]| items.iter().any(|m| matches!(m, Meta::Item { path, .. } if path.len() == 1 && ls.iter().any(|l| path[0] == l.as_bytes())));
                    if names(&["linker_messages", "linker_info"]) {
                        if !n.is_crate {
                            if a.inner { emit(&UNUSED_ATTRIBUTES, apos, "crate-level attribute should be in the root module"); }
                            else { emit(&UNUSED_ATTRIBUTES, apos, "crate-level attribute should be an inner attribute"); }
                        } else if never_needs_link(prog, krate) {
                            emit(&UNUSED_ATTRIBUTES, apos, "unused attribute");
                        }
                    } else if n.is_crate && names(&["dead_code_pub_in_binary"]) && !is_executable_crate(prog, krate) {
                        emit(&UNUSED_ATTRIBUTES, apos, "unused attribute");
                    }
                }
            }
        }
        // ---- duplicates
        let dup_name = if path.len() == 2 && path[0] == b"diagnostic" { Some(full.clone()) } else { word.clone() };
        if let Some(name) = &dup_name {
            if WARN_ON_DUPLICATE.contains(&name.as_str()) {
                if seen.iter().any(|(s, _)| s == name.as_bytes()) { emit(&UNUSED_ATTRIBUTES, apos, "unused attribute"); }
                else { seen.push((name.as_bytes().to_vec(), apos)); }
            }
            if name == "macro_use" && n.macro_use_target {
                match args {
                    Args::None => {
                        if use_all.is_some() { emit(&UNUSED_ATTRIBUTES, apos, "unused attribute"); }
                        else {
                            use_all = Some(apos);
                            for p in specific.drain(..) { emit(&UNUSED_ATTRIBUTES, p, "unused attribute"); }
                        }
                    }
                    Args::List { items, .. } => {
                        if items.is_empty() { continue; }
                        if use_all.is_some() { emit(&UNUSED_ATTRIBUTES, apos, "unused attribute"); }
                        else {
                            specific.push(apos);
                            for m in items {
                                let ok = matches!(m, Meta::Item { path, args: Args::None, .. } if path.len() == 1 && is_ident(&path[0]));
                                if !ok { emit(&MALFORMED_ATTRIBUTE, apos, "E0539: malformed `macro_use` attribute input"); }
                            }
                        }
                    }
                    Args::Value { .. } => emit(&MALFORMED_ATTRIBUTE, apos, "E0539: malformed `macro_use` attribute input"),
                }
            }
        }
        // ---- doc attributes
        if word.as_deref() == Some("doc") {
            doc_attr(prog, n, file, apos, args, &emit);
        }
    }
}

fn is_ident(t: &[u8]) -> bool {
    t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') && t.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// The crate types written as `#![crate_type = ".."]` of a crate (none: it is built as given).
fn declared_crate_types(prog: &Program, krate: u32) -> Vec<String> {
    let rm = prog.crates[krate as usize].root_mod;
    let file = prog.mods[rm as usize].file;
    prog.files[file as usize].ast.root_attrs.iter().filter(|a| lint::attr_path(prog, file, a) == b"crate_type")
        .filter_map(|a| lint::attr_text(prog, file, a).split('"').nth(1).map(str::to_string)).collect()
}

/// rustc check_attr: every crate type is `rlib` or `staticlib`.
fn never_needs_link(prog: &Program, krate: u32) -> bool {
    let t = declared_crate_types(prog, krate);
    !t.is_empty() && t.iter().all(|k| k == "lib" || k == "rlib" || k == "staticlib")
}

fn is_executable_crate(prog: &Program, krate: u32) -> bool {
    let t = declared_crate_types(prog, krate);
    t.is_empty() || t.iter().any(|k| k == "bin")
}

/// rustc_attr_parsing `DocParser`.
fn doc_attr(prog: &Program, n: &AttrNode, file: u32, apos: u32, args: &Args, emit: &dyn Fn(&Lint, u32, &str)) {
    let _ = prog;
    let invalid = |pos: u32, msg: &str| emit(&INVALID_DOC_ATTRIBUTES, pos, msg);
    let hard = |pos: u32, msg: &str| emit(&MALFORMED_ATTRIBUTE, pos, msg);
    match args {
        Args::None => {}
        Args::Value { pos, tok } => {
            // (`doc = include_str!(..)` / `concat!(..)`: a macro call, expanded to a string literal before the check)
            let literal_prefix = tok.starts_with(b"r\"") || tok.starts_with(b"r#") || tok.starts_with(b"b\"") || tok.starts_with(b"b'") || tok.starts_with(b"br");
            let macro_call = tok.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') && !literal_prefix;
            if !macro_call && string_value(tok).is_none() { invalid(*pos, "malformed `doc` attribute input"); }
        }
        Args::List { items, .. } => {
            let mut aliases: Vec<String> = Vec::new();
            let mut add_alias = |alias: String, pos: u32| {
                if alias.is_empty() { hard(pos, "`#[doc(alias = \"...\")]` attribute cannot have empty value"); return; }
                if let Some(c) = alias.chars().find(|&c| c == '"' || c == '\'' || (c.is_whitespace() && c != ' ')) {
                    hard(pos, &format!("{:?} character isn't allowed in `#[doc(alias = \"...\")]`", c));
                    return;
                }
                if alias.starts_with(' ') || alias.ends_with(' ') { hard(pos, "`#[doc(alias = \"...\")]` cannot start or end with ' '"); return; }
                if n.is_crate { hard(pos, "`#![doc(alias = \"...\")]` isn't allowed as a crate-level attribute"); return; }
                if aliases.contains(&alias) { emit(&UNUSED_ATTRIBUTES, pos, "doc alias is duplicated"); }
                aliases.push(alias);
            };
            for m in items {
                match m {
                    Meta::Lit { pos, .. } => invalid(*pos, "expected this to be of the form `... = \"...\"`"),
                    Meta::Item { pos, path, args } => {
                        let word = if path.len() == 1 { Some(String::from_utf8_lossy(&path[0]).into_owned()) } else { None };
                        let Some(name) = word else {
                            invalid(*pos, &format!("unknown `doc` attribute `{}`", joined(path)));
                            continue;
                        };
                        if name == "alias" {
                            match args {
                                Args::None => hard(*pos, "doc alias attribute expects a string `#[doc(alias = \"a\")]` or a list of strings `#[doc(alias(\"a\", \"b\"))]`"),
                                Args::Value { pos, tok } => match string_value(tok) { Some(s) => add_alias(s, *pos), None => hard(apos, "E0539: malformed `doc` attribute input") },
                                Args::List { items, .. } => for i in items {
                                    match i {
                                        Meta::Lit { pos, tok } => match string_value(tok) { Some(s) => add_alias(s, *pos), None => hard(apos, "E0539: malformed `doc` attribute input") },
                                        Meta::Item { .. } => hard(apos, "E0539: malformed `doc` attribute input"),
                                    }
                                },
                            }
                            continue;
                        }
                        if CRATE_LEVEL_DOC.contains(&name.as_str()) && !n.is_crate { invalid(*pos, "this attribute can only be applied at the crate level"); continue; }
                        if DOC_NO_ARGS.contains(&name.as_str()) && !matches!(args, Args::None) { invalid(*pos, "didn't expect any arguments here"); continue; }
                        if NOT_CRATE_LEVEL_DOC.contains(&name.as_str()) && n.is_crate { hard(*pos, &format!("`#![doc({} = \"...\")]` isn't allowed as a crate-level attribute", name)); continue; }
                        match name.as_str() {
                            "hidden" | "masked" | "notable_trait" | "inline" | "no_inline" | "html_no_source" | "fake_variadic" | "search_unbox" | "rust_logo"
                            | "cfg" | "auto_cfg" | "keyword" | "attribute" | "html_favicon_url" | "html_logo_url" | "html_playground_url"
                            | "html_root_url" | "issue_tracker_base_url" => {}
                            "test" => match args {
                                Args::List { items, .. } => for t in items {
                                    match t {
                                        Meta::Lit { pos, .. } => invalid(*pos, "malformed `doc` attribute input"),
                                        Meta::Item { pos, path, .. } => {
                                            if path.len() != 1 { invalid(*pos, "`#![doc(test(...)]` does not take a literal"); continue; }
                                            let tn = String::from_utf8_lossy(&path[0]).into_owned();
                                            if tn != "no_crate_inject" && tn != "attr" { invalid(*pos, &format!("unknown `doc(test)` attribute `{}`", tn)); }
                                        }
                                    }
                                },
                                _ => invalid(*pos, "`#[doc(test(...)]` takes a list of attributes"),
                            },
                            "spotlight" | "passes" | "no_default_passes" | "plugins" => invalid(*pos, &format!("unknown `doc` attribute `{}`", name)),
                            "include" if matches!(args, Args::Value { .. }) => invalid(*pos, "unknown `doc` attribute `include`"),
                            _ => invalid(*pos, &format!("unknown `doc` attribute `{}`", name)),
                        }
                    }
                }
            }
        }
    }
    let _ = file;
}

// ---------------------------------------------------------------- items without a definition

/// The attribute checks of the `use` and `extern crate` items of every module of the checked
/// crates (they own no definition: the module's scope gives their lint levels).
pub fn check_module_items(prog: &Program, own: &[bool]) {
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if def.kind != DefKind::Mod || !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        if prog.block_owner.get(&def.sub).is_some() { continue; }
        let d = DefId(i as u32);
        let m = &prog.mods[def.sub as usize];
        let is_root = def.parent == crate::program::NO_DEF;
        let (file, items): (u32, Vec<ItemId>) = if is_root || m.file != def.file {
            (m.file, prog.files[m.file as usize].ast.root_items.iter().copied().collect())
        } else {
            let ast = &prog.files[def.file as usize].ast;
            match ast.item(def.item).kind {
                ItemKind::Mod(Some(l)) => (def.file, ast.list(l).to_vec()),
                _ => continue,
            }
        };
        let ast = &prog.files[file as usize].ast;
        for it in items {
            let item = ast.item(it);
            if !matches!(item.kind, ItemKind::Use(_) | ItemKind::ExternCrate(_)) { continue; }
            let attrs: Vec<(u32, Attr)> = ast.list(item.attrs).iter().map(|a| (file, *a)).collect();
            let own_attrs: Vec<Attr> = ast.list(item.attrs).to_vec();
            if matches!(item.kind, ItemKind::Use(_)) { absolute_use_paths(prog, d, file, item, &own_attrs); }
            if attrs.is_empty() { continue; }
            attr_checks(prog, &AttrNode { d, inner: &own_attrs, attrs: &attrs, is_crate: false, macro_use_target: matches!(item.kind, ItemKind::ExternCrate(_)) });
        }
    }
}

// ---------------------------------------------------------------- explicit_outlives_requirements

pub const EXPLICIT_OUTLIVES_REQUIREMENTS: Lint = lint::lint("explicit_outlives_requirements", Level::Allow, &["rust_2018_idioms"]);

/// A subject of an inferred `X: 'a`: a type parameter or a lifetime parameter, by name.
#[derive(Clone, PartialEq, Eq)]
enum Subject { Type(String), Region(String) }

struct Inference<'a> {
    pf: &'a crate::parser::ParsedFile,
    lifetimes: Vec<String>,
    types: Vec<String>,
    /// the inferred `subject: 'outlived` pairs, in order
    required: Vec<(Subject, String)>,
}

impl Inference<'_> {
    fn is_free(&self, lt: &str) -> bool { lt != "'static" && self.lifetimes.iter().any(|l| l == lt) }

    /// rustc `insert_outlives_predicate` for the components of the type `t` outliving `outlived`.
    fn outlived_by(&mut self, ast: &Ast, t: TyId, outlived: &str) -> Option<()> {
        if !self.is_free(outlived) { return Some(()); }
        let mut comps: Vec<Subject> = Vec::new();
        self.components(ast, t, &mut comps)?;
        for c in comps {
            let pair = (c.clone(), outlived.to_string());
            match &c {
                Subject::Region(r) if !self.is_free(r) => continue,
                _ => {}
            }
            if !self.required.contains(&pair) { self.required.push(pair); }
        }
        Some(())
    }

    /// rustc `push_outlives_components`: the regions and parameters of a type.
    fn components(&self, ast: &Ast, t: TyId, out: &mut Vec<Subject>) -> Option<()> {
        match *ast.ty(t) {
            Ty::Ref(Some(l), _, inner) => {
                let name = self.pf.text(l).to_string();
                if name != "'static" && !self.lifetimes.contains(&name) { return None; }
                let s = Subject::Region(name);
                if !out.contains(&s) { out.push(s); }
                self.components(ast, inner, out)
            }
            Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.components(ast, x, out),
            Ty::Tuple(l) => { for &x in ast.list(l) { self.components(ast, x, out)?; } Some(()) }
            Ty::Never => Some(()),
            Ty::Path(p) => match self.leaf(ast, &p)? {
                Some(name) => { let s = Subject::Type(name); if !out.contains(&s) { out.push(s); } Some(()) }
                None => Some(()),
            },
            _ => None,
        }
    }

    /// A path that is a type parameter (Some(name)) or a primitive type (None); anything else
    /// is a type whose predicates this analysis does not model.
    fn leaf(&self, ast: &Ast, p: &Path) -> Option<Option<String>> {
        if p.has_qself() || p.global() { return None; }
        let segs = ast.segs(p.segs);
        let [seg] = segs else { return None };
        if ast.gargs(seg.args()).is_some() || seg.kind() != SegKind::Ident { return None; }
        let name = self.pf.text(seg.name).to_string();
        if self.types.contains(&name) { return Some(Some(name)); }
        const PRIMS: &[&str] = &["bool", "char", "str", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64"];
        if PRIMS.contains(&name.as_str()) { return Some(None); }
        None
    }

    /// rustc `insert_required_predicates_to_be_wf` over one field type: every reference in it.
    fn field(&mut self, ast: &Ast, t: TyId) -> Option<()> {
        match *ast.ty(t) {
            Ty::Ref(lt, _, inner) => {
                let name = self.pf.text(lt?).to_string();
                self.outlived_by(ast, inner, &name)?;
                self.field(ast, inner)
            }
            Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.field(ast, x),
            Ty::Tuple(l) => { for &x in ast.list(l) { self.field(ast, x)?; } Some(()) }
            Ty::Never => Some(()),
            Ty::Path(p) => { self.leaf(ast, &p)?; Some(()) }
            _ => None,
        }
    }
}

struct OutPred {
    in_where: bool,
    subject: Subject,
    /// the lifetime bounds' identifiers by bound index (None: a trait bound)
    bounds: Vec<Option<Ident>>,
    /// the first position of each bound
    bound_lo: Vec<u32>,
    start: u32,
}

/// rustc_lint `ExplicitOutlivesRequirements::check_item`.
fn explicit_outlives_requirements(prog: &Program, d: DefId) {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let item = *ast.item(def.item);
    let (generics, fields): (Generics, Vec<FieldDef>) = match item.kind {
        ItemKind::Struct(r) => {
            let s = *ast.row(r);
            (s.generics, match s.data { VariantData::Unit => Vec::new(), VariantData::Tuple(l) | VariantData::Struct(l) => ast.list(l).to_vec() })
        }
        ItemKind::Union(r) => { let u = *ast.row(r); (u.generics, ast.list(u.fields).to_vec()) }
        ItemKind::Enum(r) => {
            let e = *ast.row(r);
            let mut f = Vec::new();
            for v in ast.list(e.variants) { if let VariantData::Tuple(l) | VariantData::Struct(l) = v.data { f.extend_from_slice(ast.list(l)); } }
            (e.generics, f)
        }
        _ => return,
    };
    let mut inf = Inference { pf, lifetimes: Vec::new(), types: Vec::new(), required: Vec::new() };
    for p in ast.list(generics.params) {
        match p.kind {
            GenericParamKind::Lifetime(_) => inf.lifetimes.push(pf.text(p.name).to_string()),
            GenericParamKind::Type(..) => inf.types.push(pf.text(p.name).to_string()),
            GenericParamKind::Const(..) => {}
        }
    }
    if inf.lifetimes.is_empty() { return; }
    // fields of a type this analysis does not model: nothing is concluded
    for f in &fields { if inf.field(ast, f.ty).is_none() { return; } }
    if inf.required.is_empty() { return; }
    // the predicates of the generics, in rustc's HIR order: parameter bounds, then where clauses
    let mut preds: Vec<OutPred> = Vec::new();
    for p in ast.list(generics.params) {
        match p.kind {
            GenericParamKind::Lifetime(bs) if !ast.list(bs).is_empty() => {
                let bounds: Vec<Option<Ident>> = ast.list(bs).iter().map(|b| Some(*b)).collect();
                preds.push(OutPred { in_where: false, subject: Subject::Region(pf.text(p.name).to_string()), bound_lo: bounds.iter().map(|b| b.unwrap().lo).collect(), bounds, start: p.name.lo });
            }
            GenericParamKind::Type(bs, _) if !ast.list(bs).is_empty() => {
                let (bounds, bound_lo) = bound_info(ast, bs);
                preds.push(OutPred { in_where: false, subject: Subject::Type(pf.text(p.name).to_string()), bounds, bound_lo, start: p.name.lo });
            }
            _ => {}
        }
    }
    let first_where = preds.len();
    for w in ast.list(generics.where_) {
        match *w {
            WherePred::Lifetime(lt, bs) => {
                let bounds: Vec<Option<Ident>> = ast.list(bs).iter().map(|b| Some(*b)).collect();
                preds.push(OutPred { in_where: true, subject: Subject::Region(pf.text(lt).to_string()), bound_lo: bounds.iter().map(|b| b.unwrap().lo).collect(), bounds, start: lt.lo });
            }
            WherePred::Bound { ty, bounds: bs, .. } => {
                let (bounds, bound_lo) = bound_info(ast, bs);
                let start = type_start(pf, ast, ty);
                let subject = match *ast.ty(ty) {
                    Ty::Path(p) => match inf.leaf(ast, &p) { Some(Some(name)) => Subject::Type(name), _ => Subject::Region(String::new()) },
                    _ => Subject::Region(String::new()),
                };
                preds.push(OutPred { in_where: true, subject, bounds, bound_lo, start });
            }
        }
    }
    let _ = first_where;
    let num_where = preds.iter().filter(|p| p.in_where).count();
    let mut dropped_where = 0;
    let mut anchors: Vec<u32> = Vec::new();
    let mut where_anchors: Vec<u32> = Vec::new();
    for p in &preds {
        // `where` predicates that are no parameter bound are not looked at
        if let Subject::Region(r) = &p.subject { if r.is_empty() { continue; } }
        let relevant: Vec<&String> = inf.required.iter().filter(|(s, _)| *s == p.subject).map(|(_, o)| o).collect();
        if relevant.is_empty() { continue; }
        // a lifetime subject must be a declared parameter
        if let Subject::Region(r) = &p.subject { if !inf.lifetimes.contains(r) { continue; } }
        let inferred: Vec<usize> = p.bounds.iter().enumerate().filter_map(|(i, b)| {
            let name = pf.text((*b)?);
            (inf.lifetimes.iter().any(|l| l == name) && relevant.iter().any(|r| r.as_str() == name)).then_some(i)
        }).collect();
        // bounds written by an external macro are not touched
        let inferred: Vec<usize> = inferred.into_iter().filter(|&i| !lint::external(prog, def.file, p.bound_lo[i])).collect();
        let drop_predicate = inferred.len() == p.bounds.len();
        if drop_predicate && p.in_where { dropped_where += 1; }
        if inferred.is_empty() { continue; }
        if drop_predicate {
            if p.in_where { where_anchors.push(p.start); } else { anchors.push(p.start); }
        } else {
            // the first inferable bound starts its span; a later one follows its predecessor
            let i = inferred[0];
            let at = if i == 0 { p.bound_lo[0] } else { p.bound_lo[i] };
            where_anchors.push(at);
        }
    }
    if !generics.where_.is_empty() && dropped_where == num_where && num_where > 0 {
        let first = preds.iter().find(|p| p.in_where).map(|p| p.start).unwrap_or(item.lo);
        let kw = back_kw_pos(pf, first, "where");
        let tuple = matches!(item.kind, ItemKind::Struct(r) if matches!(ast.row(r).data, VariantData::Tuple(_)));
        let full = if tuple { kw } else { skip_ws_back_pos(pf, kw) };
        anchors.push(full);
    } else {
        anchors.extend(where_anchors);
    }
    let Some(pos) = anchors.iter().copied().min() else { return };
    if lint::external(prog, def.file, pos) { return; }
    emit(prog, d, &EXPLICIT_OUTLIVES_REQUIREMENTS, def.file, pos, "outlives requirements can be inferred");
}

/// The bounds of a bound list: the lifetime ones by identifier, and each bound's start.
fn bound_info(ast: &Ast, bs: List<Bound>) -> (Vec<Option<Ident>>, Vec<u32>) {
    let mut bounds = Vec::new();
    let mut lo = Vec::new();
    for b in ast.list(bs) {
        match *b {
            Bound::Lifetime(l) => { bounds.push(Some(l)); lo.push(l.lo); }
            Bound::Trait { path, .. } => { bounds.push(None); lo.push(path.lo); }
            Bound::Use(_) => { bounds.push(None); lo.push(0); }
        }
    }
    (bounds, lo)
}

fn type_start(pf: &crate::parser::ParsedFile, ast: &Ast, t: TyId) -> u32 {
    match *ast.ty(t) { Ty::Path(p) => p.lo, _ => { let _ = pf; 0 } }
}

/// The end of the token before `at` (whitespace and comments skipped).
fn skip_ws_back_pos(pf: &crate::parser::ParsedFile, at: u32) -> u32 {
    let i = pf.toks.partition_point(|t| t.lo < at);
    if i == 0 { return at; }
    pf.toks.get(i - 1).map_or(at, |t| t.hi)
}

fn back_kw_pos(pf: &crate::parser::ParsedFile, at: u32, kw: &str) -> u32 {
    let j = skip_ws_back_pos(pf, at);
    let n = kw.len() as u32;
    if j >= n && pf.bytes(j - n, j) == kw.as_bytes() { j - n } else { at }
}

// ---------------------------------------------------------------- unused_macros / unused_macro_rules

pub const UNUSED_MACROS: Lint = lint::lint("unused_macros", Level::Warn, &["unused"]);
pub const UNUSED_MACRO_RULES: Lint = lint::lint("unused_macro_rules", Level::Allow, &["unused"]);

/// rustc_resolve `check_unused_macros`, after the checked crates' bodies are all expanded: a
/// private `macro_rules!` no invocation resolved to, and the rules of the others that no
/// expansion matched.
pub fn finish(prog: &Program, own: &[bool]) {
    if !lint::enabled() { return; }
    let (defs, used) = crate::macros::uses::take();
    if defs.is_empty() { return; }
    // Bodies nothing demanded are never expanded, so an invocation inside one leaves no record:
    // a macro whose `name!` is written anywhere in the checked crates counts as used.
    let written = |name: &str| -> bool {
        let n = name.as_bytes();
        prog.files.iter().enumerate().any(|(i, f)| {
            if !own.get(prog.file_crate.get(i).copied().unwrap_or(u32::MAX) as usize).copied().unwrap_or(false) { return false; }
            let s = &f.src[..(f.src_len as usize).min(f.src.len())];
            let mut from = 0;
            while let Some(k) = s[from..].windows(n.len()).position(|w| w == n) {
                let at = from + k;
                from = at + 1;
                if at > 0 && (s[at - 1].is_ascii_alphanumeric() || s[at - 1] == b'_') { continue; }
                let mut j = at + n.len();
                while j < s.len() && s[j].is_ascii_whitespace() { j += 1; }
                if j < s.len() && s[j] == b'!' && s.get(j + 1) != Some(&b'=') { return true; }
            }
            false
        })
    };
    for d in defs {
        if d.exported || d.file.is_empty() || d.name.starts_with('_') { continue; }
        let Some(file) = prog.file_paths.iter().position(|p| *p == d.file) else { continue };
        let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let pf = &prog.files[file];
        // the definition's own node gives the lint level (a macro written by a macro has none:
        // the enclosing definition does)
        let def = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&i| {
            let x = prog.def(i);
            x.kind == DefKind::MacroRules && x.file == file as u32 && pf.ast.item(x.item).name.lo == d.pos
        }).unwrap_or_else(|| crate::lint_early::enclosing_def(prog, file as u32, d.pos, krate));
        let resolved = used.contains(&(d.file.clone(), d.pos, crate::macros::uses::RESOLVED));
        let any_rule = (0..d.rules.len() as u32).any(|i| used.contains(&(d.file.clone(), d.pos, i)));
        if !resolved && !any_rule {
            if written(&d.name) { continue; }
            if pf.expansion_kind(d.pos) == crate::parser::ExpnKind::ExternalRules { continue; }
            emit(prog, def, &UNUSED_MACROS, file as u32, d.pos, &format!("unused macro definition: `{}`", d.name));
            continue;
        }
        for (i, (pos, compile_error)) in d.rules.iter().enumerate() {
            if *compile_error || used.contains(&(d.file.clone(), d.pos, i as u32)) { continue; }
            emit(prog, def, &UNUSED_MACRO_RULES, file as u32, *pos, &format!("rule #{} of macro `{}` is never used", i + 1, d.name));
        }
    }
}

// ---------------------------------------------------------------- absolute_paths_not_starting_with_crate

pub const ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE: Lint = lint::lint("absolute_paths_not_starting_with_crate", Level::Allow, &["rust_2018_compatibility"]);
const ABSOLUTE_PATHS_MSG: &str = "absolute paths must start with `self`, `super`, `crate`, or an external crate name in the 2018 edition";

/// The crate root's `extern crate name;` items that keep their name (and the crate the compiler
/// injects): a path through one of them is already fine in the 2018 edition.
fn root_extern_crates(prog: &Program, krate: u32) -> Vec<String> {
    let rm = prog.crates[krate as usize].root_mod;
    let file = prog.mods[rm as usize].file;
    let pf = &prog.files[file as usize];
    let ast = &pf.ast;
    let mut out = vec!["std".to_string(), "core".to_string()];
    for it in ast.root_items.iter() {
        let item = ast.item(*it);
        if let ItemKind::ExternCrate(None) = item.kind { out.push(pf.text(item.name).to_string()); }
    }
    out
}

/// rustc_resolve `lint_if_path_starts_with_module` for a path written with a leading `::` in a
/// 2015 crate (expression, pattern and type paths): `::name::..` where `name` is no extern crate.
pub(crate) fn absolute_path(prog: &Program, d: DefId, inner: &[Attr], file: u32, p: &Path, times: usize) {
    let pf = &prog.files[file as usize];
    if pf.edition != 2015 || !p.global() { return; }
    let ast = &pf.ast;
    let segs = ast.segs(p.segs);
    let Some(second) = segs.first() else { return };
    // the path's first segment is the one after the `::`; the lint needs one more behind it
    if segs.len() < 1 || second.kind() != SegKind::Ident { return; }
    let krate = prog.def(d).krate;
    let name = pf.text(second.name);
    if root_extern_crates(prog, krate).iter().any(|c| c == name) { return; }
    for _ in 0..times {
        if !lint::external(prog, file, p.lo) {
            emit_in(prog, d, inner, &ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE, file, p.lo, ABSOLUTE_PATHS_MSG);
        }
    }
}

/// The same lint on a `use` item of a 2015 crate: its path implicitly starts at the crate root.
pub(crate) fn absolute_use_paths(prog: &Program, d: DefId, file: u32, item: &Item, attrs: &[Attr]) {
    let pf = &prog.files[file as usize];
    if pf.edition != 2015 { return; }
    let ast = &pf.ast;
    let ItemKind::Use(r) = item.kind else { return };
    let u = *ast.row(r);
    let krate = prog.def(d).krate;
    // where the use tree starts: the token after `use`
    let mut at = item.lo;
    let mut i = pf.toks.partition_point(|t| t.lo < item.lo);
    while let Some(t) = pf.toks.get(i) {
        if pf.bytes(t.lo, t.hi) == b"use" { at = pf.toks.get(i + 1).map_or(item.lo, |n| n.lo); break; }
        i += 1;
        if i > pf.toks.partition_point(|t| t.lo < item.lo) + 12 { break; }
    }
    fn collect(ast: &Ast, tree: UseTree, prefix: &mut Vec<(Ident, SegKind)>, out: &mut Vec<(Vec<(Ident, SegKind)>, Option<(Ident, SegKind)>, bool)>) {
        match tree {
            UseTree::Path(seg, rest) => {
                prefix.push((seg.name, seg.kind()));
                collect(ast, *ast.use_tree(rest), prefix, out);
                prefix.pop();
            }
            UseTree::Name(seg, _) => {
                if seg.kind() == SegKind::SelfValue {
                    // `foo::{self}` imports `foo`
                    if let Some(&last) = prefix.last() { out.push((prefix[..prefix.len() - 1].to_vec(), Some(last), false)); }
                } else {
                    out.push((prefix.clone(), Some((seg.name, seg.kind())), false));
                }
            }
            UseTree::Glob => out.push((prefix.clone(), None, true)),
            UseTree::Group(l) => {
                if ast.list(l).is_empty() {
                    // an empty group imports its prefix
                    if let Some(&last) = prefix.last() { out.push((prefix[..prefix.len() - 1].to_vec(), Some(last), false)); }
                }
                for t in ast.list(l) { collect(ast, *t, prefix, out); }
            }
        }
    }
    let mut leaves = Vec::new();
    collect(ast, u.tree, &mut Vec::new(), &mut leaves);
    let externs = root_extern_crates(prog, krate);
    let root = crate::lint_early::crate_root_def(prog, krate);
    for (module, ident, glob) in leaves {
        // the first segment of the full path: a keyword segment is not made absolute
        let first = module.first().map(|m| m.1).or(ident.map(|i| i.1));
        if !u.global && !matches!(first, Some(SegKind::Ident)) { continue; }
        let count = if let Some(second) = module.first() {
            // `use a::b::c`: the path resolved up to its module part, once
            let n = pf.text(second.0);
            if externs.iter().any(|c| c == n) { 0 } else { 1 }
        } else if glob || ident.is_none() {
            1
        } else {
            // `use a;`: once for each namespace `a` is bound in at the root
            let (id, _) = ident.unwrap();
            let name = pf.text(id);
            if externs.iter().any(|c| c == name) { 0 } else {
                let sym = prog.syms.intern_late(name);
                [true, false].iter().filter(|&&ns| prog.lookup_in_container(root, sym, ns).is_some()).count()
            }
        };
        for _ in 0..count {
            if !lint::external(prog, file, at) { emit_in(prog, d, attrs, &ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE, file, at, ABSOLUTE_PATHS_MSG); }
        }
    }
}

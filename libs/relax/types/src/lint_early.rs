//! Early (syntax-level) builtin lints of `makepad check`: ports of rustc_lint's EarlyLintPass
//! passes (unused parentheses / braces, `while true`, redundant semicolons, unused labels, ...).
//! Check-only.
//!
//! rustc runs these over the expanded syntax tree with the attributes of every node in scope.
//! Here the definitions of the checked crates are walked in two parts: `check_items` walks the
//! signature-level syntax of every item (generics, parameter and field types, discriminants,
//! constant initialisers' top node), `check_body` the body of a fn / const / static owner. One
//! walker (`W`) serves both; it keeps the attributes of the enclosing statements, arms, fields
//! and parameters so lint levels follow rustc's lint level builder.

use crate::ast::*;
use crate::lint::{self, Lint};
use crate::parser::{ExpnKind, ParsedFile};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::Body;
use std::collections::HashMap;

pub const UNUSED_PARENS: Lint = lint::lint("unused_parens");
pub const UNUSED_BRACES: Lint = lint::lint("unused_braces");
pub const WHILE_TRUE: Lint = lint::lint("while_true");
pub const REDUNDANT_SEMICOLONS: Lint = lint::lint("redundant_semicolons");
pub const UNUSED_LABELS: Lint = lint::lint("unused_labels");
pub const KEYWORD_IDENTS_2018: Lint = lint::lint("keyword_idents_2018");
pub const KEYWORD_IDENTS_2024: Lint = lint::lint("keyword_idents_2024");
pub const NON_ASCII_IDENTS: Lint = lint::lint("non_ascii_idents");
pub const ELLIPSIS_INCLUSIVE_RANGE_PATTERNS: Lint = lint::lint("ellipsis_inclusive_range_patterns");
pub const NO_MANGLE_CONST_ITEMS: Lint = lint::lint("no_mangle_const_items");
pub const NO_MANGLE_GENERIC_ITEMS: Lint = lint::lint("no_mangle_generic_items");
pub const SPECIAL_MODULE_NAME: Lint = lint::lint("special_module_name");
pub const UNUSED_ATTRIBUTES: Lint = lint::lint("unused_attributes");
pub const BARE_TRAIT_OBJECTS: Lint = lint::lint("bare_trait_objects");
pub const UNUSED_LIFETIMES: Lint = lint::lint("unused_lifetimes");
pub const SINGLE_USE_LIFETIMES: Lint = lint::lint("single_use_lifetimes");
pub const UNUSED_VISIBILITIES: Lint = lint::lint("unused_visibilities");

/// Item-level passes over the definitions of the checked crates marked in `own`.
pub fn check_items(prog: &Program, tcx: &Tcx, own: &[bool]) {
    if !lint::enabled() { return; }
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        if lint::generated(prog, DefId(i as u32)) { continue; }
        attr_lints(prog, DefId(i as u32));
        crate::lint_early2::check_def_item(prog, tcx, DefId(i as u32));
        if matches!(def.kind, DefKind::Fn | DefKind::ForeignFn | DefKind::Impl | DefKind::Trait | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Const | DefKind::Static) { lifetime_lints(prog, DefId(i as u32)); }
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Union
            | DefKind::Enum | DefKind::TypeAlias | DefKind::AssocTy | DefKind::ForeignTy | DefKind::Trait | DefKind::Impl
            | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic) { continue; }
        let d = DefId(i as u32);
        let mut w = W::new(prog, def.file, d);
        w.names = Some(crate::lint_early2::names(prog, tcx, d));
        w.item(def.item, def.kind);
        item_lints(prog, d);
    }
    for (k, o) in own.iter().enumerate() {
        if *o { special_module_name(prog, k as u32); }
    }
    crate::lint_early2::check_module_items(prog, own);
    token_lints(prog, own);
}

/// Body-level passes over one typed body owner.
pub fn check_body(prog: &Program, tcx: &Tcx, body: &Body) { check_def(prog, tcx, body.def) }

/// The syntax-level passes over the body of one fn / const / static definition (they need no
/// types, so they also apply to a body whose type check failed).
pub fn check_def(prog: &Program, tcx: &Tcx, def: DefId) {
    if !lint::enabled() || lint::generated(prog, def) { return; }
    let d = prog.def(def);
    let ast = &prog.files[d.file as usize].ast;
    let mut w = W::new(prog, d.file, def);
    w.names = Some(crate::lint_early2::names(prog, tcx, def));
    match ast.item(d.item).kind {
        ItemKind::Fn(..) => {
            let Ok(Some(block)) = prog.fn_body(d.file, d.item) else { return };
            w.block(block);
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            let Ok(Some(e)) = prog.const_body(d.file, d.item) else { return };
            w.expr(e);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- item attributes

/// `unused_lifetimes` / `single_use_lifetimes` (rustc_resolve late.rs `maybe_report_lifetime_uses`)
/// of the lifetime parameters of one fn, impl block or foreign fn.
fn lifetime_lints(prog: &Program, d: DefId) {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let item = *pf.ast.item(def.item);
    // a lifetime is written with a quote; the code a derive writes is not the user's
    if !pf.bytes(item.lo, item.hi).contains(&b'\'') || lint::external(prog, def.file, item.lo) { return; }
    let mut w = W::new(prog, def.file, d);
    w.lt = Some(LtCx { ribs: vec![LRib::Item], scopes: Vec::new(), next: 0 });
    match def.kind {
        DefKind::Fn | DefKind::ForeignFn => w.lt_fn(def.item),
        DefKind::Impl => w.lt_impl(d, def.item),
        DefKind::Trait => w.lt_trait(d, def.item),
        // the types of the other items hold fn pointer types, binders of their own
        DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Const | DefKind::Static => {
            let g = pf.ast.kind_generics(&item.kind).map(|g| g.params).unwrap_or(List::EMPTY);
            w.lt_scope(g, false);
            w.item(def.item, def.kind);
            if let Ok(Some(e)) = prog.const_body(def.file, def.item) {
                w.lt_rib(LRib::Fail);
                w.expr(e);
                w.lt_pop();
            }
            w.lt_end_scope();
        }
        _ => {}
    }
}

/// The name an attribute sets: `no_mangle` of `#[no_mangle]` and of `#[unsafe(no_mangle)]`.
fn attr_name(prog: &Program, file: u32, a: &Attr) -> Vec<u8> {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    if hi > lo + 2 && crate::cfg::tok_str(p, lo) == b"unsafe" && crate::cfg::tok_str(p, lo + 1) == b"(" {
        return crate::cfg::tok_str(p, lo + 2).to_vec();
    }
    lint::attr_path(prog, file, a)
}

/// rustc `InvalidNoMangleItems` (late) and the `const _` visibility check of ast_validation.
fn item_lints(prog: &Program, d: DefId) {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let item = *ast.item(def.item);
    let attrs = ast.list(item.attrs);
    match def.kind {
        DefKind::Fn | DefKind::AssocFn => {
            // `check_item` of fns and `check_impl_item`; trait methods are not checked
            let parent = prog.def(def.parent);
            if def.kind == DefKind::AssocFn && parent.kind != DefKind::Impl { return; }
            if !attrs.iter().any(|a| matches!(&attr_name(prog, def.file, a)[..], b"no_mangle" | b"export_name")) { return; }
            let ItemKind::Fn(sig, _, _) = item.kind else { return };
            let sig = *ast.fn_sig(sig);
            let mono = |g: Generics| ast.list(g.params).iter().any(|p| !matches!(p.kind, GenericParamKind::Lifetime(_)));
            let impl_trait_param = ast.list(sig.params).iter().any(|p| ty_has_impl_trait(ast, p.ty));
            let mut requires = mono(sig.generics) || impl_trait_param;
            if def.kind == DefKind::AssocFn {
                let pa = &prog.files[parent.file as usize].ast;
                if let Some(i) = pa.as_impl(&pa.item(parent.item).kind) { requires |= pa.list(i.generics.params).iter().any(|p| !matches!(p.kind, GenericParamKind::Lifetime(_))); }
            }
            if requires && !lint::external(prog, def.file, item.lo) {
                crate::lint_early2::emit(prog, d, &NO_MANGLE_GENERIC_ITEMS, def.file, item.lo, "functions generic over types or consts must be mangled");
            }
        }
        DefKind::Const => {
            if attrs.iter().any(|a| attr_name(prog, def.file, a) == b"no_mangle") && !lint::external(prog, def.file, item.lo) {
                crate::lint_early2::emit(prog, d, &NO_MANGLE_CONST_ITEMS, def.file, item.lo, "const items should never be `#[no_mangle]`");
            }
            // `pub const _: () = ..;`: the visibility has no effect
            if pf.text(item.name) == "_" && !matches!(item.vis, Vis::Private)
                && pf.hyg_ctxt(item.name.lo) == pf.hyg_ctxt(item.lo)
                && !lint::external(prog, def.file, item.lo) {
                crate::lint_early2::emit(prog, d, &UNUSED_VISIBILITIES, def.file, item.lo, "visibility qualifiers have no effect on `const _` declarations");
            }
        }
        _ => {}
    }
}

/// The position of the `#` of an attribute (its tokens are the ones inside `#[` ... `]`).
pub(crate) fn attr_pos(pf: &ParsedFile, a: &Attr) -> u32 {
    let back = if a.inner { 3 } else { 2 };
    let i = (a.toks.lo as usize).saturating_sub(back);
    pf.tok(i as u32).lo
}

/// Attribute checks of rustc_attr_parsing / rustc_passes check_attr (`unused_attributes`) on
/// the attributes of an item: crate-level attributes elsewhere than the crate root, lint
/// attributes that name no lint, `#[inline]` on bodiless functions. (`cfg_attr` is expanded by
/// the parser: an empty one is not seen here.)
fn attr_lints(prog: &Program, d: DefId) {
    const CRATE_LEVEL: &[&[u8]] = &[b"crate_name", b"crate_type", b"recursion_limit", b"type_length_limit", b"move_size_limit",
        b"pattern_complexity_limit", b"no_std", b"no_core", b"no_main", b"no_builtins", b"windows_subsystem", b"feature", b"register_tool"];
    let def = prog.def(d);
    let is_root = def.kind == DefKind::Mod && def.parent == crate::program::NO_DEF;
    let named_mod = def.kind == DefKind::Mod && def.parent != crate::program::NO_DEF && prog.block_owner.get(&def.sub).is_none();
    if !(is_root || named_mod || matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Union
        | DefKind::Enum | DefKind::TypeAlias | DefKind::AssocTy | DefKind::ForeignTy | DefKind::Trait | DefKind::Impl
        | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic | DefKind::MacroRules)) { return; }
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    // the node's attributes: the item's outer ones, then the inner ones of an out-of-line module's file
    let mut all: Vec<(u32, Attr)> = Vec::new();
    if !is_root { all.extend(ast.list(ast.item(def.item).attrs).iter().map(|a| (def.file, *a))); }
    if is_root || named_mod {
        let m = &prog.mods[def.sub as usize];
        if is_root || m.file != def.file { all.extend(prog.files[m.file as usize].ast.root_attrs.iter().map(|a| (m.file, *a))); }
    }
    let item = if is_root { None } else { Some(*ast.item(def.item)) };
    for &(file, a) in &all {
        let pf = &prog.files[file as usize];
        let name = lint::attr_path(prog, file, &a);
        let pos = attr_pos(pf, &a);
        let emit = |msg: &str| {
            if !lint::external(prog, file, pos) { crate::lint_early2::emit(prog, d, &UNUSED_ATTRIBUTES, file, pos, msg); }
        };
        if !is_root && CRATE_LEVEL.contains(&&name[..]) {
            let n = String::from_utf8_lossy(&name).into_owned();
            if a.inner { emit(&format!("the `#![{}]` attribute can only be used at the crate root", n)); }
            else { emit(&format!("crate-level attribute should be an inner attribute: add an exclamation mark: `#![{}]`", n)); }
            continue;
        }
        if name == b"inline" {
            let Some(item) = item else { continue };
            let bodiless_trait_method = def.kind == DefKind::AssocFn && prog.def(def.parent).kind == DefKind::Trait
                && matches!(item.kind, ItemKind::Fn(sig, None, _) if ast.fn_sig(sig).lazy_body.is_none());
            if bodiless_trait_method { emit("`#[inline]` attribute cannot be used on required trait methods"); }
            else if def.kind == DefKind::ForeignFn { emit("`#[inline]` attribute cannot be used on foreign functions"); }
        }
    }
    crate::lint_early2::attr_checks(prog, &crate::lint_early2::AttrNode { d, inner: &[], attrs: &all, is_crate: is_root, macro_use_target: is_root || named_mod });
}

fn ty_has_impl_trait(ast: &Ast, t: TyId) -> bool {
    match *ast.ty(t) {
        Ty::ImplTrait(_) => true,
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => ty_has_impl_trait(ast, x),
        Ty::Tuple(l) => ast.list(l).iter().any(|x| ty_has_impl_trait(ast, *x)),
        _ => false,
    }
}

/// rustc `SpecialModuleName`: `mod lib;` / `mod main;` in the crate root.
fn special_module_name(prog: &Program, krate: u32) {
    let Some(i) = (0..prog.defs.len()).find(|&i| { let d = prog.defs[i]; d.kind == DefKind::Mod && d.parent == crate::program::NO_DEF && d.krate == krate }) else { return };
    let root = DefId(i as u32);
    let m = &prog.mods[prog.def(root).sub as usize];
    let pf = &prog.files[m.file as usize];
    let ast = &pf.ast;
    for it in ast.root_items.iter() {
        let item = ast.item(*it);
        let ItemKind::Mod(None) = item.kind else { continue };
        if lint::has_attr(prog, m.file, ast.list(item.attrs), "path") { continue; }
        let msg = match pf.text(item.name) {
            "lib" => "found module declaration for lib.rs",
            "main" => "found module declaration for main.rs",
            _ => continue,
        };
        crate::lint_early2::emit(prog, root, &SPECIAL_MODULE_NAME, m.file, item.lo, msg);
    }
}

// ---------------------------------------------------------------- identifier tokens

/// rustc `KeywordIdents` and the `non_ascii_idents` part of `NonAsciiIdents`: every identifier
/// token of the checked files' own text (the lint sees the identifiers of the tree and the
/// tokens of macro definitions and calls: all of them are tokens of the source).
fn token_lints(prog: &Program, own: &[bool]) {
    use crate::lexer::T;
    let mut seen_non_ascii: std::collections::HashSet<(u32, Vec<u8>)> = std::collections::HashSet::new();
    for (f, pf) in prog.files.iter().enumerate() {
        let krate = prog.file_crate.get(f).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let edition = pf.edition;
        let n = pf.toks.len();
        // delimiters: whether each open one belongs to a macro definition or call
        let mut delims: Vec<bool> = Vec::new();
        let mut spans: Option<Vec<(u32, u32, DefId)>> = None;
        let mut root: Option<DefId> = None;
        let mut prev = T::Eof;
        for i in 0..n {
            let t = tk(pf, i);
            if t.lo >= pf.src_len { break; }
            let under_macro = delims.last().copied().unwrap_or(false);
            match t.kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => {
                    // `name!(` and `macro_rules! name {`
                    let is_call = prev == T::Not && i >= 2 && matches!(tk(pf, i - 2).kind, T::Ident | T::RawIdent | T::KwTry | T::KwAsync | T::KwAwait | T::KwDyn);
                    let is_def = prev == T::Ident && i >= 3 && tk(pf, i - 2).kind == T::Not && pf.bytes(tk(pf, i - 3).lo, tk(pf, i - 3).hi) == b"macro_rules";
                    delims.push(under_macro || is_call || is_def);
                }
                T::CloseParen | T::CloseBracket | T::CloseBrace => { delims.pop(); }
                T::Ident | T::Lifetime => {
                    let raw = pf.bytes(t.lo, t.hi);
                    let (prefix, name): (&str, &[u8]) = if t.kind == T::Lifetime { ("'", &raw[1.min(raw.len())..]) } else { ("", raw) };
                    let after_dollar = prev == T::Dollar;
                    let kw = match name {
                        b"async" | b"await" | b"try" if edition < 2018 => Some((&KEYWORD_IDENTS_2018, "2018")),
                        // `dyn` as a keyword of a type is no identifier; inside macros it is left alone
                        b"dyn" if edition < 2018 && !under_macro && !(t.kind == T::Ident && !matches!(prev, T::PathSep | T::Dot) && matches!(tk(pf, i + 1).kind, T::Ident | T::PathSep | T::KwFor | T::Question | T::Lifetime | T::OpenParen)) => Some((&KEYWORD_IDENTS_2018, "2018")),
                        b"gen" if edition < 2024 => Some((&KEYWORD_IDENTS_2024, "2024")),
                        _ => None,
                    };
                    let non_ascii = t.kind == T::Ident && !raw.is_ascii();
                    if (kw.is_some() && !after_dollar) || non_ascii {
                        let sp = spans.get_or_insert_with(|| def_spans(prog, f as u32));
                        let rt = *root.get_or_insert_with(|| crate_root(prog, krate));
                        let mut def = rt;
                        let mut best = u32::MAX;
                        for &(lo, hi, d) in sp.iter() {
                            if lo <= t.lo && t.lo < hi && hi - lo < best { best = hi - lo; def = d; }
                        }
                        if let (Some((l, ed)), false) = (kw, after_dollar) {
                            let kwname = String::from_utf8_lossy(name);
                            crate::lint_early2::emit(prog, def, l, f as u32, t.lo, &format!("`{}{}` is a keyword in the {} edition", prefix, kwname, ed));
                        }
                        if non_ascii && seen_non_ascii.insert((krate, raw.to_vec())) {
                            crate::lint_early2::emit(prog, rt, &NON_ASCII_IDENTS, f as u32, t.lo, "identifier contains non-ASCII characters");
                        }
                    }
                }
                _ => {}
            }
            prev = t.kind;
        }
    }
}

fn tk(pf: &ParsedFile, i: usize) -> crate::lexer::Tok {
    pf.toks.get(i).unwrap_or(crate::lexer::Tok { kind: crate::lexer::T::Eof, lo: u32::MAX, hi: u32::MAX })
}

/// The innermost definition whose item contains `pos` of `file` (the crate root: none does).
pub(crate) fn enclosing_def(prog: &Program, file: u32, pos: u32, krate: u32) -> DefId {
    let mut best = crate_root(prog, krate);
    let mut size = u32::MAX;
    for (lo, hi, d) in def_spans(prog, file) {
        if lo <= pos && pos < hi && hi - lo < size { size = hi - lo; best = d; }
    }
    best
}

pub(crate) fn crate_root_def(prog: &Program, krate: u32) -> DefId { crate_root(prog, krate) }

fn crate_root(prog: &Program, krate: u32) -> DefId {
    DefId((0..prog.defs.len()).find(|&i| { let d = prog.defs[i]; d.kind == DefKind::Mod && d.parent == crate::program::NO_DEF && d.krate == krate }).unwrap_or(0) as u32)
}

/// The source ranges of the items of a file's definitions.
fn def_spans(prog: &Program, file: u32) -> Vec<(u32, u32, DefId)> {
    let ast = &prog.files[file as usize].ast;
    let mut out = Vec::new();
    for i in 0..prog.defs.len() {
        let d = prog.defs[i];
        if d.file != file { continue; }
        let named_mod = d.kind == DefKind::Mod && d.parent != crate::program::NO_DEF && prog.block_owner.get(&d.sub).is_none();
        if !(named_mod || matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Union | DefKind::Enum
            | DefKind::TypeAlias | DefKind::AssocTy | DefKind::Trait | DefKind::Impl | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic)) { continue; }
        let it = ast.item(d.item);
        out.push((it.lo, it.hi, DefId(i as u32)));
    }
    out
}

// ---------------------------------------------------------------- delimiter contexts

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    FunctionArg,
    MethodArg,
    AssignedValue,
    AssignedValueLetElse,
    IfCond,
    WhileCond,
    ForIterExpr,
    MatchScrutineeExpr,
    ReturnValue,
    BlockRetValue,
    BreakValue,
    LetScrutineeExpr,
    ArrayLenExpr,
    AnonConst,
    MatchArmExpr,
    IndexExpr,
    ClosureBody,
}

impl Ctx {
    fn name(self) -> &'static str {
        match self {
            Ctx::FunctionArg => "function argument",
            Ctx::MethodArg => "method argument",
            Ctx::AssignedValue | Ctx::AssignedValueLetElse => "assigned value",
            Ctx::IfCond => "`if` condition",
            Ctx::WhileCond => "`while` condition",
            Ctx::ForIterExpr => "`for` iterator expression",
            Ctx::MatchScrutineeExpr => "`match` scrutinee expression",
            Ctx::ReturnValue => "`return` value",
            Ctx::BlockRetValue => "block return value",
            Ctx::BreakValue => "`break` value",
            Ctx::LetScrutineeExpr => "`let` scrutinee expression",
            Ctx::ArrayLenExpr | Ctx::AnonConst => "const expression",
            Ctx::MatchArmExpr => "match arm expression",
            Ctx::IndexExpr => "index expression",
            Ctx::ClosureBody => "closure body",
        }
    }
}

/// `NoBoundsException` of rustc's UnusedParens: a type in TypeNoBounds position.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nb {
    /// the type must be parenthesized
    None,
    /// the type is the last bound: parentheses around a single bound are unnecessary
    OneBound,
}

struct LabelScope {
    name: Vec<u8>,
    pos: u32,
    used: bool,
    /// a closure / async block: labels outside are out of reach
    barrier: bool,
}

enum LRib {
    /// a binder's generics (transparent)
    Generics,
    Item,
    /// the anonymous lifetimes of a binder (a fn's parameters, an impl header, a fn pointer's)
    Anon(u32),
    /// elision failure, report-only and elided ribs: a named lifetime written here is not a
    /// place an anonymous lifetime of the declaring binder could stand
    Fail,
    ImplTrait,
}

struct LtParam {
    name: Vec<u8>,
    pos: u32,
    /// 0 unused, 1 used once (where an anonymous lifetime could stand), 2 otherwise
    state: u8,
}

struct LtScope {
    params: Vec<LtParam>,
    linted: bool,
    binder: u32,
}

struct LtCx {
    ribs: Vec<LRib>,
    scopes: Vec<LtScope>,
    next: u32,
}

pub(crate) struct W<'a> {
    prog: &'a Program,
    pub(crate) pf: &'a ParsedFile,
    pub(crate) ast: &'a Ast,
    file: u32,
    def: DefId,
    /// the attributes of the enclosing statements, arms, fields, parameters, outermost first
    stack: Vec<Attr>,
    /// `1 as (i32) < 2`: the cast type nodes whose parentheses are needed
    cast_lt: Vec<u32>,
    nb: HashMap<u32, Nb>,
    self_ty_parens: bool,
    labels: Vec<LabelScope>,
    /// the last position the walk named: where a node without a position of its own is reported
    pub(crate) cur: u32,
    /// inside `&a...b`: no further `...` warnings
    ellipsis_off: bool,
    /// the lifetime use analysis (unused_lifetimes / single_use_lifetimes), when it runs
    lt: Option<LtCx>,
    /// the lifetime resolution of the definition (hidden lifetimes of paths)
    names: Option<std::sync::Arc<crate::tcx::lifetime_resolve::LifetimeResolution>>,
}

fn is_lazy(op: BinOp) -> bool { matches!(op, BinOp::And | BinOp::Or) }

impl<'a> W<'a> {
    pub(crate) fn new(prog: &'a Program, file: u32, def: DefId) -> W<'a> {
        let pf: &'a ParsedFile = &prog.files[file as usize];
        W { prog, pf, ast: &pf.ast, file, def, stack: Vec::new(), cast_lt: Vec::new(), nb: HashMap::new(), self_ty_parens: false, labels: Vec::new(), cur: 0, ellipsis_off: false, lt: None, names: None }
    }

    fn emit(&self, l: &Lint, pos: u32, msg: &str) {
        // the lifetime analysis walks the same trees: the other lints are not its business
        if self.lt.is_some() { return; }
        if lint::external(self.prog, self.file, pos) { return; }
        crate::lint_early2::emit_in(self.prog, self.def, &self.stack, l, self.file, pos, msg);
    }

    fn enter(&mut self, attrs: List<Attr>) { crate::ast_visit::push_attrs(&mut self.stack, self.ast, attrs); }
    fn leave(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }

    /// `Span::from_expansion` of the node starting at `pos`.
    fn from_exp(&self, pos: u32) -> bool { self.pf.expansion_kind(pos) != ExpnKind::Root }

    fn kind(&self, e: ExprId) -> ExprKind { self.ast.expr(e).kind }

    fn peel_parens(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Paren(x) = self.kind(e) { e = x; }
        e
    }

    // ------------------------------------------------------------ source text helpers

    /// The position just before the whitespace that precedes `at`.
    fn skip_ws_back(&self, at: u32) -> u32 {
        let mut j = at;
        while j > 0 {
            let b = self.pf.bytes(j - 1, j);
            if b.len() == 1 && matches!(b[0], b' ' | b'\t' | b'\n' | b'\r') { j -= 1; } else { break; }
        }
        j
    }

    /// The position of byte `c` right before `at` (whitespace between), else `at`.
    fn back_char(&self, at: u32, c: u8) -> u32 {
        let j = self.skip_ws_back(at);
        if j > 0 {
            let b = self.pf.bytes(j - 1, j);
            if b.len() == 1 && b[0] == c { return j - 1; }
        }
        at
    }

    /// The start of keyword `kw` right before `at`, else `at`.
    fn back_kw(&self, at: u32, kw: &str) -> u32 {
        let j = self.skip_ws_back(at);
        let n = kw.len() as u32;
        if j >= n && self.pf.bytes(j - n, j) == kw.as_bytes() { return j - n; }
        at
    }

    /// Where a pattern starts, when its tree names a position (wildcards and `..` do not).
    fn pat_start(&self, p: PatId) -> Option<u32> {
        let ast = self.ast;
        match *ast.pat(p) {
            Pat::Wild | Pat::Rest => None,
            Pat::Ident { by_ref, mutbl, name, .. } => {
                let mut at = name.lo;
                if mutbl { at = self.back_kw(at, "mut"); }
                if by_ref { at = self.back_kw(at, "ref"); }
                Some(at)
            }
            Pat::Lit(e) => Some(ast.expr(e).lo),
            Pat::Range(a, b, _) => match (a, b) {
                (Some(a), _) => Some(ast.expr(a).lo),
                (None, Some(b)) => {
                    let mut at = ast.expr(b).lo;
                    loop {
                        let j = self.skip_ws_back(at);
                        if j > 0 && matches!(self.pf.bytes(j - 1, j), [b'.'] | [b'=']) { at = j - 1; } else { break; }
                    }
                    Some(at)
                }
                _ => None,
            },
            Pat::Path(path) | Pat::TupleStruct(path, _) | Pat::Struct(path, _, _) => Some(path.lo),
            Pat::Mac(m) => Some(ast.row(m).path.lo),
            Pat::Tuple(ps) => ast.list(ps).first().and_then(|x| self.pat_start(*x)).map(|s| self.back_char(s, b'(')),
            Pat::Slice(ps) => ast.list(ps).first().and_then(|x| self.pat_start(*x)).map(|s| self.back_char(s, b'[')),
            Pat::Or(ps) => ast.list(ps).first().and_then(|x| self.pat_start(*x)),
            Pat::Ref(m, x) => self.pat_start(x).map(|s| {
                let s = if m { self.back_kw(s, "mut") } else { s };
                self.back_char(s, b'&')
            }),
            Pat::Paren(x) => self.pat_start(x).map(|s| self.back_char(s, b'(')),
            Pat::Box(x) => self.pat_start(x).map(|s| self.back_kw(s, "box")),
        }
    }

    /// The start of a bound: a parenthesized bound starts at its `(`.
    fn bound_start(&self, b: &Bound) -> Option<u32> {
        match *b {
            Bound::Lifetime(id) => Some(id.lo),
            Bound::Use(_) => None,
            Bound::Trait { path, hrtb, maybe, .. } => {
                let mut at = path.lo;
                if !self.ast.list(hrtb).is_empty() {
                    // `for<'a> `: back to the `for`
                    let j = self.skip_ws_back(at);
                    if j > 0 && self.pf.bytes(j - 1, j) == b">" {
                        let mut depth = 0i32;
                        let mut k = j;
                        while k > 0 {
                            let c = self.pf.bytes(k - 1, k);
                            match c { b">" => depth += 1, b"<" => { depth -= 1; if depth == 0 { k -= 1; break; } } _ => {} }
                            k -= 1;
                        }
                        at = self.back_kw(k, "for");
                    }
                }
                if maybe { at = self.back_char(at, b'?'); }
                Some(at)
            }
        }
    }

    /// Whether a trait bound was written in parentheses (the tree keeps no mark: the `(` right
    /// before the bound's first token).
    fn bound_parens(&self, b: &Bound) -> Option<u32> {
        let at = self.bound_start(b)?;
        let p = self.back_char(at, b'(');
        (p != at).then_some(p)
    }

    /// Where a type starts, when its tree names a position.
    pub(crate) fn ty_start(&self, t: TyId) -> Option<u32> {
        let ast = self.ast;
        match *ast.ty(t) {
            Ty::Path(p) => Some(p.lo),
            Ty::Mac(m) => Some(ast.row(m).path.lo),
            Ty::Ref(lt, m, x) => {
                let mut at = self.ty_start(x)?;
                if m { at = self.back_kw(at, "mut"); }
                if let Some(l) = lt { at = l.lo; }
                Some(self.back_char(at, b'&'))
            }
            Ty::Ptr(m, x) => {
                let at = self.ty_start(x)?;
                let at = self.back_kw(at, if m { "mut" } else { "const" });
                Some(self.back_char(at, b'*'))
            }
            Ty::Slice(x) | Ty::Array(x, _) => self.ty_start(x).map(|s| self.back_char(s, b'[')),
            Ty::Tuple(l) => ast.list(l).first().and_then(|x| self.ty_start(*x)).map(|s| self.back_char(s, b'(')),
            Ty::Paren(x) => self.ty_start(x).map(|s| self.back_char(s, b'(')),
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                // `for<'a> fn(A) -> B`, `fn(A) -> B`: from the first thing the tree names
                if let Some(p) = ast.list(f.hrtb).first() {
                    let s = self.back_char(p.name.lo, b'<');
                    return Some(self.back_kw(s, "for"));
                }
                let first = ast.list(f.params).first().and_then(|x| self.ty_start(*x));
                match first {
                    Some(s) => Some(self.back_kw(self.back_char(s, b'('), "fn")),
                    None => None,
                }
            }
            Ty::ImplTrait(b) => ast.list(b).first().and_then(|x| self.bound_start(x)).map(|s| self.back_kw(s, "impl")),
            Ty::DynTrait(b, dyn_) => ast.list(b).first().and_then(|x| self.bound_start(x)).map(|s| if dyn_ { self.back_kw(s, "dyn") } else { s }),
            _ => None,
        }
    }

    // ------------------------------------------------------------ unused_parens / unused_braces

    /// rustc `UnusedDelimLint::is_expr_delims_necessary`.
    fn necessary(&self, inner: ExprId, ctx: Ctx, followed_by_block: bool) -> bool {
        let ik = self.kind(inner);
        if ctx == Ctx::AssignedValueLetElse {
            if let ExprKind::Binary(op, ..) = ik { if is_lazy(op) { return true; } }
            if self.trailing_brace(inner) { return true; }
        }
        if matches!(ik, ExprKind::Range(..)) && ctx == Ctx::LetScrutineeExpr { return true; }
        if matches!(ik, ExprKind::AddrOf(true, ..)) { return true; }
        // the left end of a binary / call / cast / index chain is a block-like expression
        let mut innermost = inner;
        loop {
            innermost = match self.kind(innermost) {
                ExprKind::Binary(_, l, _) => l,
                ExprKind::Call(f, _) => f,
                ExprKind::Cast(e, _) => e,
                ExprKind::Index(b, _) => b,
                _ => break,
            };
            if !self.requires_semi(innermost) { return true; }
        }
        if !followed_by_block { return false; }
        // `match &( Struct { field: } ) {}`
        let mut innermost = inner;
        loop {
            match self.kind(innermost) {
                ExprKind::AddrOf(_, _, x) => innermost = x,
                _ => {
                    if self.exterior_struct_lit(innermost) { return true; }
                    break;
                }
            }
        }
        let mut innermost = inner;
        loop {
            innermost = match self.kind(innermost) {
                ExprKind::Unary(_, e) => e,
                ExprKind::Binary(_, _, r) => r,
                ExprKind::AssignOp(_, _, r) => r,
                ExprKind::Assign(_, r) => r,
                ExprKind::Return(_) | ExprKind::Yield(_) => return true,
                ExprKind::Break(_, x) => {
                    return match x.get() {
                        None => false,
                        Some(be) => matches!(self.kind(be), ExprKind::Block(..) | ExprKind::Unsafe(_) | ExprKind::Path(_)),
                    };
                }
                ExprKind::Range(_, r, _) if r.is_some() => {
                    return matches!(self.kind(r.get().unwrap()), ExprKind::Block(..) | ExprKind::Unsafe(_));
                }
                _ => return self.exterior_struct_lit(inner),
            };
        }
    }

    /// rustc `classify::expr_requires_semi_to_be_stmt`.
    fn requires_semi(&self, e: ExprId) -> bool {
        !matches!(self.kind(e), ExprKind::If(..) | ExprKind::Match(..) | ExprKind::Block(..) | ExprKind::Unsafe(_)
            | ExprKind::While(_) | ExprKind::Loop(..) | ExprKind::For(_) | ExprKind::ConstBlock(_))
    }

    /// rustc `classify::expr_trailing_brace(e).is_some()`.
    fn trailing_brace(&self, mut e: ExprId) -> bool {
        loop {
            e = match self.kind(e) {
                ExprKind::AddrOf(_, _, x) | ExprKind::Assign(_, x) | ExprKind::AssignOp(_, _, x)
                | ExprKind::Binary(_, _, x) | ExprKind::Unary(_, x) | ExprKind::Let(_, x) => x,
                ExprKind::Break(_, x) | ExprKind::Return(x) | ExprKind::Yield(x) | ExprKind::Range(_, x, _) => match x.get() {
                    Some(x) => x,
                    None => return false,
                },
                ExprKind::Closure(r) => self.ast.row(r).body,
                ExprKind::Block(..) | ExprKind::Unsafe(_) | ExprKind::Async(..) | ExprKind::ConstBlock(_) | ExprKind::For(_)
                | ExprKind::If(..) | ExprKind::Loop(..) | ExprKind::Match(..) | ExprKind::Struct(_) | ExprKind::While(_) => return true,
                _ => return false,
            };
        }
    }

    /// rustc `parser::contains_exterior_struct_lit`.
    fn exterior_struct_lit(&self, e: ExprId) -> bool {
        match self.kind(e) {
            ExprKind::Struct(_) => true,
            ExprKind::Assign(l, r) | ExprKind::AssignOp(_, l, r) | ExprKind::Binary(_, l, r) => self.exterior_struct_lit(l) || self.exterior_struct_lit(r),
            ExprKind::Await(x) | ExprKind::Unary(_, x) | ExprKind::Cast(x, _) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Index(x, _) => self.exterior_struct_lit(x),
            ExprKind::MethodCall(recv, _) => self.exterior_struct_lit(recv),
            _ => false,
        }
    }

    fn emit_delims(&self, l: &Lint, pos: u32, delim: &str, item: &str) {
        self.emit(l, pos, &format!("unnecessary {} around {}", delim, item));
    }

    /// rustc `UnusedDelimLint::check_unused_delims_expr` of UnusedParens (`parens`) or UnusedBraces.
    fn delims(&self, parens: bool, value: ExprId, ctx: Ctx, followed_by_block: bool) {
        let v = *self.ast.expr(value);
        if parens {
            match v.kind {
                ExprKind::Paren(inner) => {
                    if !self.necessary(inner, ctx, followed_by_block)
                        && !self.from_exp(v.lo)
                        && (ctx != Ctx::LetScrutineeExpr || !matches!(self.kind(inner), ExprKind::Binary(op, ..) if is_lazy(op)))
                        && !((ctx == Ctx::ReturnValue || ctx == Ctx::BreakValue) && matches!(self.kind(inner), ExprKind::Assign(..)))
                    {
                        self.emit_delims(&UNUSED_PARENS, v.lo, "parentheses", ctx.name());
                    }
                }
                ExprKind::Let(_, x) => self.delims(parens, x, Ctx::LetScrutineeExpr, followed_by_block),
                _ => {}
            }
        } else {
            match v.kind {
                ExprKind::Block(b, label) if label.is_none() => {
                    let blk = *self.ast.block(b);
                    let stmts = self.ast.stmts(b);
                    if stmts.len() != 1 || !self.ast.list(blk.attrs).is_empty() { return; }
                    let expr = match stmts[0] {
                        Stmt::Expr(e, false) => e,
                        Stmt::Attrs(r) => {
                            let a = *self.ast.row(r);
                            if a.semi { return; }
                            a.expr
                        }
                        _ => return,
                    };
                    let ex = self.ast.expr(expr);
                    if !self.necessary(expr, ctx, followed_by_block)
                        && (ctx != Ctx::AnonConst || (matches!(ex.kind, ExprKind::Lit(_)) && !self.from_exp(ex.lo)))
                        && ctx != Ctx::ClosureBody
                        && !self.pf.bytes(v.lo, v.hi).contains(&b'\n')
                        && !self.from_exp(v.lo)
                        && !self.from_exp(blk.lo)
                        // braces substituted into a macro_rules body (`Ok($b)` with `$b:block`) are the
                        // macro's: rustc never lints them
                        && !(v.lo >= self.pf.src_len && matches!(self.pf.expansion_region_kind(v.lo), ExpnKind::LocalRules | ExpnKind::ExternalRules))
                    {
                        self.emit_delims(&UNUSED_BRACES, v.lo, "braces", ctx.name());
                    }
                }
                ExprKind::Let(_, x) => self.delims(parens, x, Ctx::LetScrutineeExpr, followed_by_block),
                _ => {}
            }
        }
    }

    /// Both delimiter lints on one value.
    fn both(&self, value: ExprId, ctx: Ctx, fbb: bool) {
        self.delims(true, value, ctx, fbb);
        self.delims(false, value, ctx, fbb);
    }

    /// rustc `UnusedParens::check_unused_parens_pat`.
    fn pat_parens(&self, value: PatId, avoid_or: bool, avoid_mut: bool, fallback: u32) {
        let Pat::Paren(inner) = *self.ast.pat(value) else { return };
        match *self.ast.pat(inner) {
            Pat::Range(..) => return,
            Pat::Or(_) if avoid_or => return,
            Pat::Ident { by_ref: false, mutbl: true, .. } if avoid_mut => return,
            _ => {}
        }
        let pos = self.pat_start(value).unwrap_or(fallback);
        self.emit_delims(&UNUSED_PARENS, pos, "parentheses", "pattern");
    }

    /// rustc `UnusedParens::check_pat`: the parenthesized sub-patterns of a pattern.
    fn pat_check(&self, p: PatId, fallback: u32) {
        let ast = self.ast;
        let at = self.pat_start(p).unwrap_or(fallback);
        match *ast.pat(p) {
            Pat::TupleStruct(_, ps) | Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => {
                for &x in ast.list(ps) { self.pat_parens(x, false, false, at); }
            }
            Pat::Struct(_, fs, _) => {
                for f in ast.list(fs) { self.pat_parens(f.pat, false, false, at); }
            }
            Pat::Ident { sub: Some(x), .. } | Pat::Box(x) => self.pat_parens(x, true, false, at),
            Pat::Ref(m, x) => self.pat_parens(x, true, !m, at),
            _ => {}
        }
    }

    /// rustc `cast_followed_by_lt`: the parenthesized cast type of `a as (T) < b`.
    fn cast_followed_by_lt(&self, e: ExprId) -> Option<u32> {
        if let ExprKind::Binary(op, lhs, _) = self.kind(e) {
            if matches!(op, BinOp::Lt | BinOp::Shl) {
                let mut cur = lhs;
                while let ExprKind::Binary(_, _, r) = self.kind(cur) { cur = r; }
                if let ExprKind::Cast(_, ty) = self.kind(cur) {
                    if matches!(*self.ast.ty(ty), Ty::Paren(_)) { return Some(ty.0); }
                }
            }
        }
        None
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self, e: ExprId) {
        let ast = self.ast;
        let ex = *ast.expr(e);
        self.cur = ex.lo;
        let cast_lt = self.cast_followed_by_lt(e);
        if let Some(t) = cast_lt { self.cast_lt.push(t); }
        self.expr_pre(e, ex);
        self.expr_children(e, ex);
        if cast_lt.is_some() { self.cast_lt.pop(); }
    }

    /// The generic `UnusedDelimLint::check_expr` of both lints, UnusedParens' own cases and
    /// UnusedBraces' repeat length, and the `while true` lint.
    fn expr_pre(&mut self, e: ExprId, ex: Expr) {
        let ast = self.ast;
        // while_true
        if let ExprKind::While(r) = ex.kind {
            let w = *ast.row(r);
            let c = self.peel_parens(w.cond);
            if matches!(self.kind(c), ExprKind::Lit(LitKind::Bool(true))) && !self.from_exp(ast.expr(w.cond).lo) {
                self.emit(&WHILE_TRUE, ex.lo, "denote infinite loops with `loop { ... }`");
            }
        }
        // UnusedParens::check_expr (own part)
        let mut skip_generic_parens = false;
        match ex.kind {
            ExprKind::Let(pat, _) => self.pat_parens_in(pat, ex.lo),
            ExprKind::For(r) => self.pat_parens_in(ast.row(r).pat, ex.lo),
            ExprKind::If(cond, _, _) if matches!(self.kind(self.peel_parens(cond)), ExprKind::Let(..)) => {
                self.delims(true, self.peel_parens(cond), Ctx::LetScrutineeExpr, true);
                skip_generic_parens = true;
            }
            ExprKind::Match(_, arms) => {
                for a in ast.list(arms) { self.delims(true, a.body, Ctx::MatchArmExpr, false); }
            }
            _ => {}
        }
        // UnusedDelimLint::check_expr, for UnusedParens (`parens`) and UnusedBraces
        for parens in [true, false] {
            if parens && skip_generic_parens { continue; }
            let pat_ctx = parens;
            let (value, ctx, fbb) = match ex.kind {
                ExprKind::If(cond, _, _) if !matches!(self.kind(cond), ExprKind::Let(..)) || pat_ctx => (cond, Ctx::IfCond, true),
                ExprKind::While(r) if !matches!(self.kind(ast.row(r).cond), ExprKind::Let(..)) || pat_ctx => (ast.row(r).cond, Ctx::WhileCond, true),
                ExprKind::For(r) => (ast.row(r).iter, Ctx::ForIterExpr, true),
                ExprKind::Match(x, _) if pat_ctx => (x, Ctx::MatchScrutineeExpr, true),
                ExprKind::Return(x) if x.is_some() => (x.get().unwrap(), Ctx::ReturnValue, false),
                ExprKind::Break(label, x) if x.is_some() => {
                    let v = x.get().unwrap();
                    if label.is_some() {
                        if let ExprKind::Paren(inner) = self.kind(v) {
                            if matches!(self.kind(inner), ExprKind::Block(..)) { continue; }
                        }
                    }
                    (v, Ctx::BreakValue, false)
                }
                ExprKind::Index(_, v) => (v, Ctx::IndexExpr, false),
                ExprKind::Assign(_, v) | ExprKind::AssignOp(_, _, v) => (v, Ctx::AssignedValue, false),
                ExprKind::Call(_, args) => {
                    for &a in ast.list(args) { self.delims(parens, a, Ctx::FunctionArg, false); }
                    continue;
                }
                ExprKind::MethodCall(_, row) => {
                    for &a in ast.list(ast.row(row).args) { self.delims(parens, a, Ctx::MethodArg, false); }
                    continue;
                }
                ExprKind::Closure(r) if ast.row(r).ret.is_none() => {
                    self.delims(parens, ast.row(r).body, Ctx::ClosureBody, false);
                    continue;
                }
                _ => continue,
            };
            self.delims(parens, value, ctx, fbb);
        }
        // UnusedBraces: `[x; { 3 }]`
        if let ExprKind::Repeat(_, len) = ex.kind { self.delims(false, len, Ctx::AnonConst, false); }
        let _ = e;
    }

    /// The parenthesized sub-pattern check of a `let` / `for` pattern (`avoid_or` off, spaces kept).
    fn pat_parens_in(&self, pat: PatId, fallback: u32) { self.pat_parens(pat, false, false, fallback) }

    fn exprs(&mut self, l: List<ExprId>) {
        let ast = self.ast;
        for &x in ast.list(l) { self.expr(x); }
    }

    fn expr_children(&mut self, _e: ExprId, ex: Expr) {
        let ast = self.ast;
        match ex.kind {
            ExprKind::Lit(_) | ExprKind::Underscore => {}
            ExprKind::Path(p) => self.path(&p),
            ExprKind::Continue(l) => self.use_label(l),
            ExprKind::Unary(_, x) | ExprKind::Paren(x) | ExprKind::Box(x) | ExprKind::Try(x) | ExprKind::Await(x)
            | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::AddrOf(_, _, x) => self.expr(x),
            ExprKind::Cast(x, t) => { self.expr(x); self.ty(t); }
            ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Repeat(a, b) => { self.expr(a); self.expr(b); }
            ExprKind::Call(f, args) => { self.expr(f); self.exprs(args); }
            ExprKind::MethodCall(recv, row) => {
                let m = *ast.row(row);
                self.expr(recv);
                self.generic_args(m.turbofish);
                self.exprs(m.args);
            }
            ExprKind::Block(b, label) => self.labeled(label, |w| w.block(b)),
            ExprKind::Loop(b, label) => self.labeled(label, |w| w.block(b)),
            ExprKind::Unsafe(b) | ExprKind::ConstBlock(b) => self.block(b),
            ExprKind::Async(_, b) => {
                self.labels.push(LabelScope { name: Vec::new(), pos: 0, used: true, barrier: true });
                self.block(b);
                self.labels.pop();
            }
            ExprKind::If(c, then, els) => {
                self.expr(c);
                self.block(then);
                if let Some(x) = els.get() { self.expr(x); }
            }
            ExprKind::Let(p, x) => {
                self.pat(p, ex.lo);
                self.expr(x);
            }
            ExprKind::Match(x, arms) => {
                self.expr(x);
                for a in ast.list(arms) {
                    self.enter(a.attrs);
                    self.pat_parens(a.pat, false, false, ex.lo);
                    self.pat(a.pat, ex.lo);
                    if let Some(g) = a.guard { self.expr(g); }
                    self.expr(a.body);
                    self.leave(a.attrs);
                }
            }
            ExprKind::While(r) => {
                let w = *ast.row(r);
                self.labeled(OptIdent::from(w.label), |s| {
                    s.expr(w.cond);
                    s.block(w.body);
                });
            }
            ExprKind::For(r) => {
                let f = *ast.row(r);
                self.expr(f.iter);
                self.pat(f.pat, ex.lo);
                self.labeled(OptIdent::from(f.label), |s| s.block(f.body));
            }
            ExprKind::Break(l, x) => {
                self.use_label(l);
                if let Some(x) = x.get() { self.expr(x); }
            }
            ExprKind::Return(x) | ExprKind::Yield(x) => if let Some(x) = x.get() { self.expr(x); },
            ExprKind::Closure(row) => {
                let c = *ast.row(row);
                self.labels.push(LabelScope { name: Vec::new(), pos: 0, used: true, barrier: true });
                for p in ast.list(c.params) {
                    self.enter(p.attrs);
                    self.pat_parens(p.pat, true, false, ex.lo);
                    self.pat(p.pat, ex.lo);
                    if let Some(t) = p.ty { self.ty(t); }
                    self.leave(p.attrs);
                }
                if let Some(t) = c.ret { self.ty(t); }
                self.expr(c.body);
                self.labels.pop();
            }
            ExprKind::Tuple(xs) | ExprKind::Array(xs) => self.exprs(xs),
            ExprKind::Struct(row) => {
                let s = *ast.row(row);
                self.path(&s.path);
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
                if let Some(p) = args.pat { self.pat(p, ex.lo); }
                if let Some(g) = args.guard { self.expr(g); }
            }
        }
    }

    // ------------------------------------------------------------ labels (unused_labels)

    /// rustc_resolve late.rs `unused_labels`: a loop or block label that no `break` / `continue`
    /// inside it names.
    fn labeled(&mut self, label: OptIdent, body: impl FnOnce(&mut W<'a>)) {
        let Some(l) = label.get() else { return body(self) };
        let name = self.pf.bytes(l.lo, l.hi).to_vec();
        // a label starting with `_` is never reported
        let used = name.get(1) == Some(&b'_');
        self.labels.push(LabelScope { name, pos: l.lo, used, barrier: false });
        let n = self.labels.len();
        body(self);
        let sc = self.labels.pop().unwrap();
        debug_assert_eq!(self.labels.len() + 1, n);
        if !sc.used { self.emit(&UNUSED_LABELS, sc.pos, "unused label"); }
    }

    fn use_label(&mut self, label: OptIdent) {
        let Some(l) = label.get() else { return };
        let name = self.pf.bytes(l.lo, l.hi);
        for sc in self.labels.iter_mut().rev() {
            if sc.barrier { break; }
            if sc.name == name && self.pf.hyg_visible(sc.pos, l.lo) {
                sc.used = true;
                return;
            }
        }
    }

    // ------------------------------------------------------------ statements and blocks

    fn block(&mut self, b: BlockId) {
        let ast = self.ast;
        let blk = *ast.block(b);
        self.enter(blk.attrs);
        self.redundant_semicolons(b, &blk);
        for s in ast.stmts(b) { self.stmt(s, &blk); }
        self.leave(blk.attrs);
    }

    fn stmt(&mut self, s: &Stmt, blk: &Block) {
        let ast = self.ast;
        match *s {
            Stmt::Let(r) => {
                let l = *ast.row(r);
                self.enter(l.attrs);
                // UnusedParens::check_stmt
                self.pat_parens(l.pat, true, false, blk.lo);
                if let Some(init) = l.init {
                    let els = l.else_.is_some();
                    let skip = els && match self.kind(init) {
                        ExprKind::Paren(inner) => self.pf.hyg_ctxt(ast.expr(init).lo) != self.pf.hyg_ctxt(ast.expr(inner).lo),
                        _ => false,
                    };
                    if !skip { self.delims(true, init, if els { Ctx::AssignedValueLetElse } else { Ctx::AssignedValue }, false); }
                }
                self.pat(l.pat, blk.lo);
                if let Some(t) = l.ty { self.ty(t); }
                if let Some(x) = l.init { self.expr(x); }
                if let Some(e) = l.else_ { self.block(e); }
                self.leave(l.attrs);
            }
            Stmt::Expr(e, semi) => {
                // the lint attributes of an expression statement survive cfg stripping
                let attrs = ast.stmt_attrs(e);
                self.enter(attrs);
                // (`#[cfg(..)]` stays on the node in rustc: a value with attributes is not linted)
                if !semi && attrs.len() == 0 { self.both(e, Ctx::BlockRetValue, false); }
                self.expr(e);
                self.leave(attrs);
            }
            Stmt::Attrs(r) => {
                let a = *ast.row(r);
                self.enter(a.attrs);
                if !a.semi { self.both(a.expr, Ctx::BlockRetValue, false); }
                self.expr(a.expr);
                self.leave(a.attrs);
            }
            Stmt::Item(it) => {
                let item = *ast.item(it);
                if matches!(item.kind, ItemKind::Use(_)) && self.lt.is_none() {
                    crate::lint_early2::absolute_use_paths(self.prog, self.def, self.file, &item, &ast.list(item.attrs).to_vec());
                }
            }
            Stmt::Empty => {}
        }
    }

    /// rustc `redundant_semicolon.rs`: a run of empty statements.
    fn redundant_semicolons(&self, b: BlockId, blk: &Block) {
        let ast = self.ast;
        let stmts = ast.stmts(b);
        let mut i = 0;
        while i < stmts.len() {
            if !matches!(stmts[i], Stmt::Empty) { i += 1; continue; }
            let start = i;
            while i < stmts.len() && matches!(stmts[i], Stmt::Empty) { i += 1; }
            let run = i - start;
            let lower = if start == 0 { blk.lo + 1 } else { self.stmt_end(&stmts[start - 1]).unwrap_or(blk.lo + 1) };
            let upper = stmts[i..].iter().find_map(|s| self.stmt_start(s)).unwrap_or(blk.hi.saturating_sub(1));
            let lower = if lower < blk.lo { blk.lo + 1 } else { lower };
            // only whitespace, comments and `;` between the neighbours: anything else is a
            // statement the tree dropped (a cfg-stripped item)
            let Some(pos) = self.nth_last_semi(lower, upper, run) else { continue };
            let msg = if run > 1 { "unnecessary trailing semicolons" } else { "unnecessary trailing semicolon" };
            self.emit(&REDUNDANT_SEMICOLONS, pos, msg);
        }
    }

    /// The position of the `run`-th last `;` in the text [lower, upper).
    fn nth_last_semi(&self, lower: u32, upper: u32, run: usize) -> Option<u32> {
        if upper <= lower { return None; }
        let text = self.pf.bytes(lower, upper);
        let mut semis = Vec::new();
        let mut i = 0;
        while i < text.len() {
            match text[i] {
                b' ' | b'\t' | b'\n' | b'\r' => i += 1,
                b';' => { semis.push(lower + i as u32); i += 1; }
                b'/' if text.get(i + 1) == Some(&b'/') => {
                    while i < text.len() && text[i] != b'\n' { i += 1; }
                }
                b'/' if text.get(i + 1) == Some(&b'*') => {
                    match text[i + 2..].windows(2).position(|w| w == b"*/") {
                        Some(e) => i += e + 4,
                        None => return None,
                    }
                }
                _ => return None,
            }
        }
        // the run, plus possibly the terminator of the statement before it
        if semis.len() < run || semis.len() > run + 1 { return None; }
        Some(semis[semis.len() - run])
    }

    fn stmt_start(&self, s: &Stmt) -> Option<u32> {
        let ast = self.ast;
        match *s {
            Stmt::Expr(e, _) => Some(ast.expr(e).lo),
            Stmt::Attrs(r) => Some(ast.expr(ast.row(r).expr).lo),
            Stmt::Item(i) => Some(ast.item(i).lo),
            Stmt::Let(r) => self.pat_start(ast.row(r).pat),
            Stmt::Empty => None,
        }
    }

    fn stmt_end(&self, s: &Stmt) -> Option<u32> {
        let ast = self.ast;
        match *s {
            Stmt::Expr(e, _) => Some(ast.expr(e).hi),
            Stmt::Attrs(r) => Some(ast.expr(ast.row(r).expr).hi),
            Stmt::Item(i) => Some(ast.item(i).hi),
            Stmt::Let(r) => {
                let l = *ast.row(r);
                if let Some(e) = l.else_ { return Some(ast.block(e).hi); }
                if let Some(x) = l.init { return Some(ast.expr(x).hi); }
                self.pat_start(l.pat)
            }
            Stmt::Empty => None,
        }
    }

    // ------------------------------------------------------------ patterns

    /// rustc `EllipsisInclusiveRangePatterns`: `a...b`, reported at the `...` (at the `&` of
    /// `&a...b`, and then not inside the endpoints).
    fn ellipsis_pat(&mut self, p: PatId, fallback: u32) -> bool {
        let ast = self.ast;
        if self.ellipsis_off { return false; }
        let (parens, range) = match *ast.pat(p) {
            Pat::Ref(_, x) => (true, *ast.pat(x)),
            other => (false, other),
        };
        let Pat::Range(a, Some(b), true) = range else { return false };
        // the tree keeps no mark of the spelling: the text between the endpoints
        let from = match a { Some(a) => ast.expr(a).hi, None => self.pat_start(p).unwrap_or(fallback) };
        let to = ast.expr(b).lo;
        if to <= from { return false; }
        let text = self.pf.bytes(from, to);
        let Some(off) = text.windows(3).position(|w| w == b"...") else { return false };
        let pos = if parens { self.pat_start(p).unwrap_or(fallback) } else { from + off as u32 };
        if self.pf.edition < 2021 {
            self.emit(&ELLIPSIS_INCLUSIVE_RANGE_PATTERNS, pos, "`...` range patterns are deprecated");
        }
        parens
    }

    fn pat(&mut self, p: PatId, fallback: u32) {
        let ast = self.ast;
        let off = self.ellipsis_pat(p, fallback);
        if off {
            self.ellipsis_off = true;
            self.pat_inner(p, fallback);
            self.ellipsis_off = false;
        } else {
            self.pat_inner(p, fallback);
        }
        let _ = ast;
    }

    fn pat_inner(&mut self, p: PatId, fallback: u32) {
        let ast = self.ast;
        self.pat_check(p, fallback);
        let at = self.pat_start(p).unwrap_or(fallback);
        match *ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Mac(_) => {}
            Pat::Ident { sub, .. } => if let Some(s) = sub { self.pat(s, at) },
            Pat::Lit(e) => self.expr(e),
            Pat::Range(a, b, _) => {
                if let Some(x) = a { self.expr(x); }
                if let Some(x) = b { self.expr(x); }
            }
            Pat::Path(path) => self.path(&path),
            Pat::TupleStruct(path, ps) => {
                self.path(&path);
                for &x in ast.list(ps) { self.pat(x, at); }
            }
            Pat::Struct(path, fs, _) => {
                self.path(&path);
                for f in ast.list(fs) {
                    self.enter(f.attrs);
                    self.pat(f.pat, at);
                    self.leave(f.attrs);
                }
            }
            Pat::Tuple(ps) | Pat::Slice(ps) | Pat::Or(ps) => for &x in ast.list(ps) { self.pat(x, at); },
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat(x, at),
        }
    }

    // ------------------------------------------------------------ types, paths, generics

    fn ty(&mut self, t: TyId) {
        let ast = self.ast;
        let ty = *ast.ty(t);
        self.ty_pre(t, ty);
        match ty {
            Ty::Path(p) => self.path(&p),
            Ty::Ref(lt, _, x) => {
                if let Some(l) = lt { self.lt_use(l); }
                self.ty(x);
            }
            Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Paren(x) => self.ty(x),
            Ty::Array(x, e) => {
                self.ty(x);
                self.expr(e);
            }
            Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x); },
            Ty::Fn(f) => {
                let f = *ast.fn_ptr(f);
                // a binder of its own: the parameters are its anonymous lifetimes
                let binder = self.lt_scope(f.hrtb, true);
                self.generic_params(f.hrtb);
                self.lt_rib(LRib::Anon(binder));
                for &x in ast.list(f.params) { self.ty(x); }
                self.lt_rib(LRib::Fail);
                if let Some(r) = f.ret { self.ty(r); }
                self.lt_pop();
                self.lt_pop();
                self.lt_end_scope();
            }
            Ty::ImplTrait(b) => {
                self.lt_rib(LRib::ImplTrait);
                self.bounds(b);
                self.lt_pop();
            }
            Ty::DynTrait(b, _) => self.bounds(b),
            Ty::Never | Ty::Infer | Ty::Mac(_) | Ty::ImplicitSelf | Ty::CVarArgs => {}
        }
    }

    /// `check_ty` of UnusedParens and UnusedBraces.
    fn ty_pre(&mut self, t: TyId, ty: Ty) {
        let ast = self.ast;
        // bare trait objects (edition 2021 rejects them)
        if let Ty::DynTrait(_, false) = ty {
            if self.pf.edition < 2021 {
                let pos = self.ty_start(t).unwrap_or(self.cur);
                self.emit(&BARE_TRAIT_OBJECTS, pos, "trait objects without an explicit `dyn` are deprecated");
            }
        }
        // UnusedBraces
        if let Ty::Array(_, len) = ty { self.delims(false, len, Ctx::ArrayLenExpr, false); }
        // UnusedParens
        if matches!(ty, Ty::Paren(_)) && self.cast_lt.last() == Some(&t.0) { return; }
        match ty {
            Ty::Array(_, len) => self.delims(true, len, Ctx::ArrayLenExpr, false),
            Ty::Paren(r) => {
                let unused = match *ast.ty(r) {
                    Ty::ImplTrait(b) | Ty::DynTrait(b, _) => match self.nb.get(&t.0) {
                        Some(Nb::None) => false,
                        Some(Nb::OneBound) => ast.list(b).len() <= 1,
                        None => true,
                    },
                    Ty::Fn(f) => !self.self_ty_parens || ast.list(ast.fn_ptr(f).hrtb).is_empty(),
                    _ => true,
                };
                if unused {
                    let pos = self.ty_start(t).or_else(|| self.ty_start(r)).unwrap_or(self.cur);
                    self.emit_delims(&UNUSED_PARENS, pos, "parentheses", "type");
                }
                self.self_ty_parens = false;
            }
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) => {
                let constraint = match self.nb.get(&t.0) {
                    Some(Nb::None) => Nb::None,
                    _ => Nb::OneBound,
                };
                self.nb.insert(x.0, constraint);
            }
            Ty::ImplTrait(bl) | Ty::DynTrait(bl, _) => {
                let bounds = ast.list(bl);
                let is_dyn = matches!(ty, Ty::DynTrait(..));
                for (i, b) in bounds.iter().enumerate() {
                    let is_last = i == bounds.len() - 1;
                    let Bound::Trait { path, .. } = *b else { continue };
                    let mut fn_with_explicit_ret_ty = false;
                    if let Some(seg) = ast.segs(path.segs).last() {
                        if let Some(GenericArgs::Paren(_, Some(ret))) = ast.gargs(seg.args()) {
                            self.nb.insert(ret.0, if is_last { Nb::OneBound } else { Nb::None });
                            fn_with_explicit_ret_ty = true;
                        }
                    }
                    let dyn2015_exception = self.pf.edition == 2015 && is_dyn && i == 0 && path.global();
                    if let Some(pos) = self.bound_parens(b) {
                        if (is_last || !fn_with_explicit_ret_ty) && !dyn2015_exception {
                            self.emit_delims(&UNUSED_PARENS, pos, "parentheses", "type");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn bounds(&mut self, l: List<Bound>) {
        let ast = self.ast;
        for b in ast.list(l) { self.bound(b); }
    }

    fn bound(&mut self, b: &Bound) {
        match *b {
            Bound::Lifetime(l) => self.lt_use(l),
            Bound::Trait { hrtb, path, .. } => {
                // a poly trait ref is a binder (not one the lints report)
                self.lt_scope(hrtb, false);
                self.generic_params(hrtb);
                self.path(&path);
                self.lt_end_scope();
            }
            Bound::Use(_) => {}
        }
    }

    fn path(&mut self, p: &Path) {
        let ast = self.ast;
        if p.has_qself() {
            if let Some(q) = ast.qself(p) {
                self.ty(q.ty);
                if let Some(tp) = q.trait_path {
                    self.path(&tp);
                    // rustc resolves the trait part of a qualified path twice
                    if self.lt.is_none() { crate::lint_early2::absolute_path(self.prog, self.def, &self.stack, self.file, &tp, 1); }
                }
            }
        }
        for seg in ast.segs(p.segs) { self.generic_args(seg.args()); }
        self.hidden_lifetimes(p);
        if lint::enabled() && self.lt.is_none() { crate::lint_early2::absolute_path(self.prog, self.def, &self.stack, self.file, p, 1); }
    }

    /// rustc_resolve `resolve_elided_lifetimes_in_path`: `elided_lifetimes_in_paths` for each
    /// segment of a type path that names a type with lifetime parameters and gives none (the
    /// paths of expressions and patterns infer them).
    fn hidden_lifetimes(&self, p: &Path) {
        use crate::tcx::lifetime_resolve::LifetimeRes;
        let Some(names) = &self.names else { return };
        let ast = self.ast;
        for (i, seg) in ast.segs(p.segs).iter().enumerate() {
            match names.path(p.segs.lo + i as u32, 0) {
                None | Some(LifetimeRes::Infer) | Some(LifetimeRes::Error) => continue,
                Some(_) => {}
            }
            // with generic arguments written (`Foo<>`, `Foo<u8>`) the opening `<`, else the name
            let mut pos = seg.name.lo;
            if ast.gargs(seg.args()).is_some() {
                let mut at = seg.name.hi;
                let n = self.pf.src_len;
                while at < n && self.pf.bytes(at, at + 1) != b"<" { at += 1; }
                if at < n { pos = at; }
            }
            self.emit(&crate::lint_early2::ELIDED_LIFETIMES_IN_PATHS, pos, "hidden lifetime parameters in types are deprecated");
        }
    }

    fn generic_args(&mut self, id: GenericArgsId) {
        let ast = self.ast;
        let Some(g) = ast.gargs(id) else { return };
        match *g {
            GenericArgs::Angle(l) => {
                for a in ast.list(l) {
                    match *a {
                        GenericArg::Lifetime(l) => self.lt_use(l),
                        GenericArg::Type(t) => self.ty(t),
                        GenericArg::Const(e) => {
                            self.delims(false, e, Ctx::AnonConst, false);
                            self.expr(e);
                        }
                        GenericArg::Binding(_, ga, t) => {
                            self.generic_args(ga);
                            self.ty(t);
                        }
                        GenericArg::Constraint(_, _, b) => self.bounds(b),
                    }
                }
            }
            GenericArgs::Paren(tys, ret) => {
                // `Fn(A) -> B`: the inputs are the anonymous lifetimes of a binder of the sugar
                let binder = self.lt_fresh();
                self.lt_rib(LRib::Anon(binder));
                for &t in ast.list(tys) { self.ty(t); }
                self.lt_pop();
                self.lt_rib(LRib::Fail);
                if let Some(r) = ret { self.ty(r); }
                self.lt_pop();
            }
        }
    }

    fn generic_params(&mut self, l: List<GenericParam>) {
        let ast = self.ast;
        for p in ast.list(l) {
            self.enter(p.attrs);
            match p.kind {
                GenericParamKind::Lifetime(bounds) => for &l in ast.list(bounds) { self.lt_use(l); },
                GenericParamKind::Type(b, d) => {
                    self.bounds(b);
                    if let Some(t) = d { self.ty(t); }
                }
                GenericParamKind::Const(t, d) => {
                    self.ty(t);
                    if let Some(e) = d { self.expr(e); }
                }
            }
            self.leave(p.attrs);
        }
    }

    fn generics(&mut self, g: Generics) {
        let ast = self.ast;
        self.generic_params(g.params);
        for w in ast.list(g.where_) {
            match *w {
                WherePred::Bound { hrtb, ty, bounds } => {
                    if matches!(*ast.ty(ty), Ty::Paren(_)) && ast.list(hrtb).is_empty() { self.self_ty_parens = true; }
                    self.lt_scope(hrtb, true);
                    self.generic_params(hrtb);
                    self.ty(ty);
                    self.bounds(bounds);
                    self.lt_end_scope();
                }
                WherePred::Lifetime(l, bounds) => {
                    self.lt_use(l);
                    for &b in ast.list(bounds) { self.lt_use(b); }
                }
            }
        }
    }

    // ------------------------------------------------------------ lifetime uses

    fn lt_fresh(&mut self) -> u32 {
        match self.lt.as_mut() {
            Some(c) => { c.next += 1; c.next }
            None => 0,
        }
    }

    fn lt_rib(&mut self, r: LRib) { if let Some(c) = self.lt.as_mut() { c.ribs.push(r); } }
    fn lt_pop(&mut self) { if let Some(c) = self.lt.as_mut() { c.ribs.pop(); } }

    /// Opens a binder declaring the lifetime parameters of `params`: its scope and its generics
    /// rib. Returns the binder.
    fn lt_scope(&mut self, params: List<GenericParam>, linted: bool) -> u32 {
        if self.lt.is_none() { return 0; }
        let binder = self.lt_fresh();
        let ast = self.ast;
        let mut ps = Vec::new();
        for p in ast.list(params) {
            if let GenericParamKind::Lifetime(_) = p.kind {
                ps.push(LtParam { name: self.pf.bytes(p.name.lo, p.name.hi).to_vec(), pos: p.name.lo, state: 0 });
            }
        }
        let c = self.lt.as_mut().unwrap();
        c.scopes.push(LtScope { params: ps, linted, binder });
        c.ribs.push(LRib::Generics);
        binder
    }

    /// Closes the innermost binder's scope: its unused and single-use parameters are reported.
    /// (Its generics rib is popped here too.)
    fn lt_end_scope(&mut self) {
        let Some(c) = self.lt.as_mut() else { return };
        c.ribs.pop();
        let Some(scope) = c.scopes.pop() else { return };
        if !scope.linted { return; }
        for p in scope.params {
            let name = String::from_utf8_lossy(&p.name).into_owned();
            match p.state {
                0 => {
                    // a parameter written by a macro cannot be removed: no report
                    if self.pf.expansion_kind(p.pos) == ExpnKind::Root {
                        crate::lint_early2::emit_in(self.prog, self.def, &self.stack, &UNUSED_LIFETIMES, self.file, p.pos, &format!("lifetime parameter `{}` never used", name));
                    }
                }
                1 => {
                    if !lint::external(self.prog, self.file, p.pos) {
                        crate::lint_early2::emit_in(self.prog, self.def, &self.stack, &SINGLE_USE_LIFETIMES, self.file, p.pos, &format!("lifetime parameter `{}` only used once", name));
                    }
                }
                _ => {}
            }
        }
    }

    /// rustc_resolve `resolve_lifetime`: a use of a named lifetime. The first use of a parameter
    /// is a single use when the innermost rib that decides says an anonymous lifetime could stand
    /// in its place (the parameters of the fn / impl / fn pointer type that declares it); any
    /// other use makes it used many times.
    fn lt_use(&mut self, id: Ident) {
        let Some(c) = self.lt.as_mut() else { return };
        let name = self.pf.bytes(id.lo, id.hi);
        let Some(si) = c.scopes.iter().rposition(|s| s.params.iter().any(|p| p.name == name)) else { return };
        let binder = c.scopes[si].binder;
        let pj = c.scopes[si].params.iter().position(|p| p.name == name).unwrap();
        if c.scopes[si].params[pj].state != 0 {
            c.scopes[si].params[pj].state = 2;
            return;
        }
        let mut state = 2;
        for r in c.ribs.iter().rev() {
            match r {
                LRib::Generics => continue,
                LRib::Anon(b) => { state = if *b == binder { 1 } else { 2 }; break; }
                LRib::Item | LRib::Fail | LRib::ImplTrait => { state = 2; break; }
            }
        }
        c.scopes[si].params[pj].state = state;
    }

    /// The lifetime analysis of one fn: its generics, signature and body.
    fn lt_fn(&mut self, id: ItemId) {
        let ast = self.ast;
        let item = *ast.item(id);
        let ItemKind::Fn(sig_id, _, _) = item.kind else { return };
        let sig = *ast.fn_sig(sig_id);
        let binder = self.lt_scope(sig.generics.params, true);
        self.generics(sig.generics);
        self.lt_rib(LRib::Anon(binder));
        if let Some(sp) = sig.self_param {
            match sp {
                SelfParam::Ref(Some(l), _) => self.lt_use(l),
                SelfParam::Typed(_, t) => self.ty(t),
                _ => {}
            }
        }
        for p in ast.list(sig.params) {
            self.pat(p.pat, item.lo);
            self.ty(p.ty);
        }
        self.lt_rib(LRib::Fail);
        if let Some(r) = sig.ret { self.ty(r); }
        self.lt_pop();
        self.lt_pop();
        // the body: lifetimes written in it are used many times
        self.lt_rib(LRib::Fail);
        if let Ok(Some(b)) = self.prog.fn_body(self.file, id) { self.block(b); }
        self.lt_pop();
        self.lt_end_scope();
    }

    /// The lifetime analysis of an impl block: its header, generics and associated items.
    fn lt_impl(&mut self, d: DefId, id: ItemId) {
        let ast = self.ast;
        let ItemKind::Impl(r) = ast.item(id).kind else { return };
        let i = *ast.row(r);
        let binder = self.lt_scope(i.generics.params, true);
        self.lt_rib(LRib::Anon(binder));
        if let Some(p) = i.trait_ { self.path(&p); }
        self.ty(i.self_ty);
        self.generics(i.generics);
        let outer = self.def;
        for &it in ast.list(i.items) {
            let child = (0..self.prog.defs.len()).map(|k| DefId(k as u32)).find(|&k| { let c = self.prog.def(k); c.parent == d && c.file == self.file && c.item == it });
            if let Some(c) = child { self.def = c; }
            match ast.item(it).kind {
                ItemKind::Fn(..) => self.lt_fn(it),
                ItemKind::Const(t, _) => {
                    self.lt_rib(LRib::Fail);
                    if let Some(t) = t.get() { self.ty(t); }
                    if let Ok(Some(e)) = self.prog.const_body(self.file, it) { self.expr(e); }
                    self.lt_pop();
                }
                ItemKind::TypeAlias(tr) => {
                    self.lt_rib(LRib::Fail);
                    let a = *ast.row(tr);
                    self.generics(a.generics);
                    if let Some(t) = a.ty { self.ty(t); }
                    self.lt_pop();
                }
                _ => {}
            }
            self.def = outer;
        }
        self.lt_pop();
        self.lt_end_scope();
    }

    /// The lifetime analysis of a trait: its methods (the trait's own lifetimes are no
    /// business of the lints).
    fn lt_trait(&mut self, d: DefId, id: ItemId) {
        let ast = self.ast;
        let ItemKind::Trait(r) = ast.item(id).kind else { return };
        let t = *ast.row(r);
        self.lt_scope(t.generics.params, false);
        let outer = self.def;
        for &it in ast.list(t.items) {
            if !matches!(ast.item(it).kind, ItemKind::Fn(..)) { continue; }
            let child = (0..self.prog.defs.len()).map(|k| DefId(k as u32)).find(|&k| { let c = self.prog.def(k); c.parent == d && c.file == self.file && c.item == it });
            if let Some(c) = child { self.def = c; }
            self.lt_fn(it);
            self.def = outer;
        }
        self.lt_end_scope();
    }

    // ------------------------------------------------------------ items

    fn fields(&mut self, l: List<FieldDef>) {
        let ast = self.ast;
        for f in ast.list(l) {
            self.enter(f.attrs);
            self.ty(f.ty);
            if let Some(d) = f.default { self.expr(d); }
            self.leave(f.attrs);
        }
    }

    fn variant_data(&mut self, d: VariantData) {
        match d {
            VariantData::Unit => {}
            VariantData::Tuple(l) | VariantData::Struct(l) => self.fields(l),
        }
    }

    /// The signature-level syntax of the item of a definition.
    fn item(&mut self, id: ItemId, kind: DefKind) {
        let ast = self.ast;
        let item = *ast.item(id);
        self.cur = item.lo;
        match item.kind {
            ItemKind::Fn(sig, _, _) => {
                let sig = *ast.fn_sig(sig);
                self.generics(sig.generics);
                if let Some(SelfParam::Typed(_, t)) = sig.self_param { self.ty(t); }
                for p in ast.list(sig.params) {
                    self.enter(p.attrs);
                    self.pat_parens(p.pat, true, false, item.lo);
                    self.pat(p.pat, item.lo);
                    self.ty(p.ty);
                    self.leave(p.attrs);
                }
                if let Some(r) = sig.ret { self.ty(r); }
            }
            ItemKind::Struct(r) => {
                let s = *ast.row(r);
                self.generics(s.generics);
                self.variant_data(s.data);
            }
            ItemKind::Union(r) => {
                let u = *ast.row(r);
                self.generics(u.generics);
                self.fields(u.fields);
            }
            ItemKind::Enum(r) => {
                let en = *ast.row(r);
                self.generics(en.generics);
                for v in ast.list(en.variants) {
                    self.enter(v.attrs);
                    self.variant_data(v.data);
                    if let Some(d) = v.disc {
                        self.delims(false, d, Ctx::AnonConst, false);
                        self.expr(d);
                    }
                    self.leave(v.attrs);
                }
            }
            ItemKind::TypeAlias(r) => {
                let a = *ast.row(r);
                self.generics(a.generics);
                self.bounds(a.bounds);
                if let Some(t) = a.ty { self.ty(t); }
            }
            ItemKind::Const(t, v) => {
                if let Some(t) = t.get() { self.ty(t); }
                if kind == DefKind::Const { if let Some(e) = v.get() { self.both(e, Ctx::AssignedValue, false); } }
            }
            ItemKind::Static(_, t, v) => {
                self.ty(t);
                if kind == DefKind::Static { if let Some(e) = v.get() { self.both(e, Ctx::AssignedValue, false); } }
            }
            ItemKind::Trait(r) => {
                let t = *ast.row(r);
                self.generics(t.generics);
                self.bounds(t.supers);
            }
            ItemKind::Impl(r) => {
                let i = *ast.row(r);
                self.generics(i.generics);
                if let Some(p) = i.trait_ { self.path(&p); }
                self.ty(i.self_ty);
            }
            _ => {}
        }
    }
}

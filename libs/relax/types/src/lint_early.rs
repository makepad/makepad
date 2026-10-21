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
use crate::tcx::lifetime_resolve::{LifetimeRes, LifetimeResolution};
use crate::tcx::Tcx;
use std::sync::Arc;
use crate::typeck::Body;
use std::collections::HashMap;

pub const UNUSED_IMPORT_BRACES: Lint = lint::lint("unused_import_braces");
pub const BREAK_WITH_LABEL_AND_LOOP: Lint = lint::lint("break_with_label_and_loop");
pub const AMBIGUOUS_NEGATIVE_LITERALS: Lint = lint::lint("ambiguous_negative_literals");
pub const DROP_BOUNDS: Lint = lint::lint("drop_bounds");
pub const DYN_DROP: Lint = lint::lint("dyn_drop");
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
        check_def_item(prog, tcx, DefId(i as u32));
        if matches!(def.kind, DefKind::Fn | DefKind::ForeignFn | DefKind::Impl | DefKind::Trait | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Const | DefKind::Static) { lifetime_lints(prog, DefId(i as u32)); }
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Union
            | DefKind::Enum | DefKind::TypeAlias | DefKind::AssocTy | DefKind::ForeignTy | DefKind::Trait | DefKind::Impl
            | DefKind::Const | DefKind::Static | DefKind::AssocConst | DefKind::ForeignStatic) { continue; }
        let d = DefId(i as u32);
        let mut w = W::new(prog, def.file, d);
        w.names = Some(names(prog, tcx, d));
        w.item(def.item, def.kind);
        item_lints(prog, d);
    }
    for (k, o) in own.iter().enumerate() {
        if *o { special_module_name(prog, k as u32); }
    }
    check_module_items(prog, own);
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
    w.names = Some(names(prog, tcx, def));
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
                emit(prog, d, &NO_MANGLE_GENERIC_ITEMS, def.file, item.lo, "functions generic over types or consts must be mangled");
            }
        }
        DefKind::Const => {
            if attrs.iter().any(|a| attr_name(prog, def.file, a) == b"no_mangle") && !lint::external(prog, def.file, item.lo) {
                emit(prog, d, &NO_MANGLE_CONST_ITEMS, def.file, item.lo, "const items should never be `#[no_mangle]`");
            }
            // `pub const _: () = ..;`: the visibility has no effect
            if pf.text(item.name) == "_" && !matches!(item.vis, Vis::Private)
                && pf.hyg_ctxt(item.name.lo) == pf.hyg_ctxt(item.lo)
                && !lint::external(prog, def.file, item.lo) {
                emit(prog, d, &UNUSED_VISIBILITIES, def.file, item.lo, "visibility qualifiers have no effect on `const _` declarations");
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
            if !lint::external(prog, file, pos) { emit(prog, d, &UNUSED_ATTRIBUTES, file, pos, msg); }
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
    attr_checks(prog, &AttrNode { d, inner: &[], attrs: &all, is_crate: is_root, macro_use_target: is_root || named_mod });
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
        emit(prog, root, &SPECIAL_MODULE_NAME, m.file, item.lo, msg);
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
                            emit(prog, def, l, f as u32, t.lo, &format!("`{}{}` is a keyword in the {} edition", prefix, kwname, ed));
                        }
                        if non_ascii && seen_non_ascii.insert((krate, raw.to_vec())) {
                            emit(prog, rt, &NON_ASCII_IDENTS, f as u32, t.lo, "identifier contains non-ASCII characters");
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
    /// the type a bound under walk constrains (`T` in `T: Drop`), for the `drop_bounds` text
    bound_self: String,
    /// inside `dyn ..` bounds / the return type (an opaque `impl Drop` is allowed)
    in_dyn: bool,
    in_ret: bool,
}

fn is_lazy(op: BinOp) -> bool { matches!(op, BinOp::And | BinOp::Or) }

impl<'a> W<'a> {
    pub(crate) fn new(prog: &'a Program, file: u32, def: DefId) -> W<'a> {
        let pf: &'a ParsedFile = &prog.files[file as usize];
        W { prog, pf, ast: &pf.ast, file, def, stack: Vec::new(), cast_lt: Vec::new(), nb: HashMap::new(), self_ty_parens: false, labels: Vec::new(), cur: 0, ellipsis_off: false, lt: None, names: None, bound_self: String::new(), in_dyn: false, in_ret: false }
    }

    fn emit(&self, l: &Lint, pos: u32, msg: &str) {
        // the lifetime analysis walks the same trees: the other lints are not its business
        if self.lt.is_some() { return; }
        if lint::external(self.prog, self.file, pos) { return; }
        emit_in(self.prog, self.def, &self.stack, l, self.file, pos, msg);
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
    fn delims(&self, parens: bool, mut value: ExprId, ctx: Ctx, followed_by_block: bool) {
        // the parens a macro transcription adds around an `$e:expr` fragment are no node of
        // rustc's (the fragment is one interpolated expression)
        while let ExprKind::Paren(inner) = self.kind(value) {
            if !crate::macros::uses::is_synth_paren(&self.prog.file_paths[self.file as usize], self.ast.expr(value).lo) { break; }
            value = inner;
        }
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

    /// rustc `Precedence`: `-1i32.abs()` negates the method call's result.
    fn ambiguous_negative_literal(&self, _e: ExprId, ex: Expr, operand: ExprId) {
        let mut arg = operand;
        let mut at_least_one = false;
        while let ExprKind::MethodCall(recv, _) = self.kind(arg) {
            at_least_one = true;
            arg = recv;
        }
        if at_least_one {
            if let ExprKind::Lit(LitKind::Int | LitKind::Float) = self.kind(arg) {
                self.emit(&AMBIGUOUS_NEGATIVE_LITERALS, ex.lo, "`-` has lower precedence than method calls, which might be unexpected");
            }
        }
    }

    fn expr(&mut self, e: ExprId) {
        let ast = self.ast;
        let ex = *ast.expr(e);
        self.cur = ex.lo;
        if let ExprKind::Unary(UnOp::Neg, operand) = ex.kind { self.ambiguous_negative_literal(e, ex, operand); }
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
                if let Some(x) = x.get() {
                    // rustc_parse: `break 'l loop {..}` reads as an unlabeled break of a labeled value
                    if !l.is_none() {
                        let bare = match self.kind(x) {
                            ExprKind::While(r) => ast.row(r).label.is_none(),
                            ExprKind::For(r) => ast.row(r).label.is_none(),
                            ExprKind::Loop(_, lb) => lb.is_none(),
                            ExprKind::Block(_, lb) => lb.is_none(),
                            _ => false,
                        };
                        if bare && !lint::external(self.prog, self.file, ex.lo) {
                            let krate = self.prog.def(self.def).krate;
                            let root = crate_root_def(self.prog, krate);
                            emit(self.prog, root, &BREAK_WITH_LABEL_AND_LOOP, self.file, ex.lo, "this labeled break expression is easy to confuse with an unlabeled break with a labeled value expression");
                        }
                    }
                    self.expr(x);
                }
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
                    absolute_use_paths(self.prog, self.def, self.file, &item, &ast.list(item.attrs).to_vec());
                    unused_import_braces(self.prog, self.def, self.file, &item, &ast.list(item.attrs).to_vec());
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
                let s = std::mem::replace(&mut self.bound_self, "impl Drop".to_string());
                self.bounds(b);
                self.bound_self = s;
                self.lt_pop();
            }
            Ty::DynTrait(b, _) => { let s = std::mem::replace(&mut self.in_dyn, true); self.bounds(b); self.in_dyn = s; }
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
                if let Some(d) = crate::lint_dead::resolve_trait_path(self.prog, self.def, &path) {
                    if lint::is_diagnostic_item(self.prog, d, "Drop") {
                        if self.in_dyn {
                            self.emit(&DYN_DROP, path.lo, "types that do not implement `Drop` can still have drop glue, consider instead using `std::mem::needs_drop` to detect whether a type is trivially dropped");
                        } else if !(self.in_ret && self.bound_self == "impl Drop") {
                            let ty = self.bound_self.clone();
                            self.emit(&DROP_BOUNDS, path.lo, &format!("bounds on `{}: Drop` are most likely incorrect, consider instead using `std::mem::needs_drop::<{}>()` to detect whether a type is trivially dropped", ty, ty));
                        }
                    }
                }
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
                    if self.lt.is_none() { absolute_path(self.prog, self.def, &self.stack, self.file, &tp, 1); }
                }
            }
        }
        for seg in ast.segs(p.segs) { self.generic_args(seg.args()); }
        self.hidden_lifetimes(p);
        if lint::enabled() && self.lt.is_none() { absolute_path(self.prog, self.def, &self.stack, self.file, p, 1); }
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
            self.emit(&ELIDED_LIFETIMES_IN_PATHS, pos, "hidden lifetime parameters in types are deprecated");
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
                    let s = std::mem::replace(&mut self.bound_self, self.pf.text(p.name).to_string());
                    self.bounds(b);
                    self.bound_self = s;
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
                    let text = match *ast.ty(ty) {
                        Ty::Path(p) => crate::program::path_segs(self.prog, self.file, &p).iter().map(|s| self.prog.syms.str(s.1).to_string()).collect::<Vec<_>>().join("::"),
                        _ => "_".to_string(),
                    };
                    let s = std::mem::replace(&mut self.bound_self, text);
                    self.bounds(bounds);
                    self.bound_self = s;
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
                        emit_in(self.prog, self.def, &self.stack, &UNUSED_LIFETIMES, self.file, p.pos, &format!("lifetime parameter `{}` never used", name));
                    }
                }
                1 => {
                    if !lint::external(self.prog, self.file, p.pos) {
                        emit_in(self.prog, self.def, &self.stack, &SINGLE_USE_LIFETIMES, self.file, p.pos, &format!("lifetime parameter `{}` only used once", name));
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
        if let Some(r) = sig.ret { let s = std::mem::replace(&mut self.in_ret, true); self.ty(r); self.in_ret = s; }
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
                if let Some(r) = sig.ret { let s = std::mem::replace(&mut self.in_ret, true); self.ty(r); self.in_ret = s; }
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
                let s = std::mem::replace(&mut self.bound_self, "Self".to_string());
                self.bounds(t.supers);
                self.bound_self = s;
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

// ================================================================ lints that need more than one node
//
// These use the lifetime resolution (`mismatched_lifetime_syntaxes`, `elided_lifetimes_in_paths`),
// the macro definitions, or the source text between tokens.

pub const MISMATCHED_LIFETIME_SYNTAXES: Lint = lint::lint("mismatched_lifetime_syntaxes");
pub const ELIDED_LIFETIMES_IN_PATHS: Lint = lint::lint("elided_lifetimes_in_paths");

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

// ---------------------------------------------------------------- impl_trait_redundant_captures

pub const IMPL_TRAIT_REDUNDANT_CAPTURES: Lint = lint::lint("impl_trait_redundant_captures");

/// The `impl Trait` types written in `t` (outermost first), with the bounds each holds.
fn opaque_types(ast: &Ast, t: TyId, out: &mut Vec<TyId>) {
    fn bounds(ast: &Ast, l: List<Bound>, out: &mut Vec<TyId>) {
        for b in ast.list(l) {
            if let Bound::Trait { path, .. } = *b { path_args(ast, &path, out); }
        }
    }
    fn path_args(ast: &Ast, p: &Path, out: &mut Vec<TyId>) {
        if p.has_qself() {
            if let Some(q) = ast.qself(p) {
                opaque_types(ast, q.ty, out);
                if let Some(tp) = q.trait_path { path_args(ast, &tp, out); }
            }
        }
        for seg in ast.segs(p.segs) {
            match ast.gargs(seg.args()) {
                Some(GenericArgs::Angle(l)) => {
                    for a in ast.list(*l) {
                        match *a {
                            GenericArg::Type(t) | GenericArg::Binding(_, _, t) => opaque_types(ast, t, out),
                            GenericArg::Constraint(_, _, b) => bounds(ast, b, out),
                            _ => {}
                        }
                    }
                }
                Some(GenericArgs::Paren(tys, ret)) => {
                    for &t in ast.list(*tys) { opaque_types(ast, t, out); }
                    if let Some(r) = ret { opaque_types(ast, *r, out); }
                }
                None => {}
            }
        }
    }
    match *ast.ty(t) {
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => opaque_types(ast, x, out),
        Ty::Tuple(l) => for &x in ast.list(l) { opaque_types(ast, x, out); },
        Ty::Fn(f) => {
            let f = *ast.fn_ptr(f);
            for &x in ast.list(f.params) { opaque_types(ast, x, out); }
            if let Some(r) = f.ret { opaque_types(ast, r, out); }
        }
        Ty::Path(p) => path_args(ast, &p, out),
        Ty::ImplTrait(b) => { out.push(t); bounds(ast, b, out); }
        Ty::DynTrait(b, _) => bounds(ast, b, out),
        _ => {}
    }
}

/// rustc_lint impl_trait_overcaptures.rs `check_fn`, the `use<..>` redundancy: in edition 2024 an
/// opaque type in a fn's return type whose `use<..>` names every generic parameter and lifetime in
/// scope (a parameter-position `impl Trait` is one that cannot be named).
fn impl_trait_redundant_captures(prog: &Program, tcx: &Tcx, d: DefId) {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    if pf.edition < 2024 { return; }
    let ast = &pf.ast;
    let item = *ast.item(def.item);
    let ItemKind::Fn(sig, _, _) = item.kind else { return };
    let sig = *ast.fn_sig(sig);
    let Some(ret) = sig.ret else { return };
    let mut opaques = Vec::new();
    opaque_types(ast, ret, &mut opaques);
    if !opaques.iter().any(|&t| matches!(*ast.ty(t), Ty::ImplTrait(b) if ast.list(b).iter().any(|x| matches!(x, Bound::Use(_))))) { return; }
    // a parameter-position `impl Trait` is a generic parameter nothing can capture by name
    if ast.list(sig.params).iter().any(|p| ty_has_impl_trait(ast, p.ty)) { return; }
    if let Some(SelfParam::Typed(_, t)) = sig.self_param { if ty_has_impl_trait(ast, t) { return; } }
    // the type and const parameters in scope, by name; `Self` in a trait
    let mut params: Vec<String> = Vec::new();
    fn add(params: &mut Vec<String>, ast: &Ast, pf: &ParsedFile, g: Generics) {
        for p in ast.list(g.params) {
            if !matches!(p.kind, GenericParamKind::Lifetime(_)) { params.push(pf.text(p.name).to_string()); }
        }
    }
    add(&mut params, ast, pf, sig.generics);
    if def.kind == DefKind::AssocFn {
        let parent = prog.def(def.parent);
        let ppf = &prog.files[parent.file as usize];
        let past = &ppf.ast;
        match past.item(parent.item).kind {
            ItemKind::Impl(r) => add(&mut params, past, ppf, past.row(r).generics),
            ItemKind::Trait(r) => { params.push("Self".to_string()); add(&mut params, past, ppf, past.row(r).generics) }
            _ => {}
        }
    }
    let names = names(prog, tcx, d);
    let scope: Vec<crate::tcx::resolve_bound_vars::LifetimeId> = names.in_scope_lifetimes();
    let w = W::new(prog, def.file, d);
    for t in opaques {
        let Ty::ImplTrait(b) = *ast.ty(t) else { continue };
        let Some(args) = ast.list(b).iter().find_map(|x| if let Bound::Use(a) = *x { Some(a) } else { None }) else { continue };
        let Some(GenericArgs::Angle(l)) = ast.gargs(args) else { continue };
        let mut lifetimes = Vec::new();
        let mut named: Vec<String> = Vec::new();
        for a in ast.list(*l) {
            match *a {
                GenericArg::Lifetime(lt) => if let Some(LifetimeRes::Param(id)) = names.named(lt.lo, lt.hi) { lifetimes.push(id) },
                GenericArg::Type(ty) => if let Ty::Path(p) = *ast.ty(ty) {
                    if let Some(sg) = ast.segs(p.segs).last() { named.push(pf.text(sg.name).to_string()); }
                },
                GenericArg::Const(e) => if let ExprKind::Path(p) = ast.expr(e).kind {
                    if let Some(sg) = ast.segs(p.segs).last() { named.push(pf.text(sg.name).to_string()); }
                },
                _ => {}
            }
        }
        if scope.iter().all(|id| lifetimes.contains(id)) && params.iter().all(|p| named.contains(p)) {
            let pos = w.ty_start(t).unwrap_or(item.lo);
            if lint::external(prog, def.file, pos) { continue; }
            emit(prog, d, &IMPL_TRAIT_REDUNDANT_CAPTURES, def.file, pos, "all possible in-scope parameters are already captured, so `use<...>` syntax is redundant");
        }
    }
}

// ---------------------------------------------------------------- impl_trait_overcaptures

pub const IMPL_TRAIT_OVERCAPTURES: Lint = lint::lint("impl_trait_overcaptures");

/// rustc `ty::Variance` of a lifetime in a signature, as `V`.
type Var = crate::types::variance::V;

/// The variance of the lifetimes in the signature `sig` of `d` (rustc `FunctionalVariances`):
/// arguments contravariant, the return type covariant; an `impl Trait` captures its lifetimes
/// invariantly. A lifetime that does not occur has no entry.
fn sig_lifetime_variances(prog: &Program, tcx: &Tcx, d: DefId, nm: &LifetimeResolution, captured: &HashMap<u32, Vec<crate::types::LifetimeId>>) -> HashMap<crate::types::LifetimeId, Var> {
    use crate::types::variance::{join, xform, V};
    struct Cx<'a> { prog: &'a Program, tcx: &'a Tcx, module: u32, ast: &'a Ast, pf: &'a ParsedFile, nm: &'a LifetimeResolution, captured: &'a HashMap<u32, Vec<crate::types::LifetimeId>>, out: HashMap<crate::types::LifetimeId, V> }
    impl Cx<'_> {
        fn record(&mut self, r: Option<LifetimeRes>, v: V) {
            if let Some(LifetimeRes::Param(id)) = r {
                let e = self.out.entry(id).or_insert(V::Bi);
                *e = join(*e, v);
            }
        }
        fn inv(&mut self, t: TyId, ctx: V) {
            // everything inside is invariant
            self.walk(t, xform(ctx, V::Inv));
        }
        fn walk(&mut self, t: TyId, ctx: V) {
            let ast = self.ast;
            match *ast.ty(t) {
                Ty::Ref(lt, m, x) => {
                    let r = match lt { Some(l) => self.nm.named(l.lo, l.hi), None => self.nm.reference(t.0) };
                    self.record(r, xform(ctx, V::Co));
                    self.walk(x, xform(ctx, if m { V::Inv } else { V::Co }));
                }
                Ty::Ptr(m, x) => self.walk(x, xform(ctx, if m { V::Inv } else { V::Co })),
                Ty::Slice(x) | Ty::Paren(x) => self.walk(x, ctx),
                Ty::Array(x, _) => self.walk(x, ctx),
                Ty::Tuple(l) => for &x in ast.list(l) { self.walk(x, ctx); },
                Ty::Fn(f) => {
                    let f = *ast.fn_ptr(f);
                    for &x in ast.list(f.params) { self.walk(x, xform(ctx, V::Contra)); }
                    if let Some(r) = f.ret { self.walk(r, ctx); }
                }
                Ty::Path(p) => self.path(p, ctx),
                Ty::ImplTrait(b) => {
                    // an opaque type captures its lifetimes invariantly
                    if let Some(ids) = self.prog.node_id_to_def_id.get(&(self.pf_index(), relax_front::def_collector::NodeId::OpaqueTy(t))).and_then(|o| self.captured.get(&o.0)) {
                        for &id in ids.clone().iter() { self.record(Some(LifetimeRes::Param(id)), V::Inv); }
                    }
                    self.bounds(b, ctx);
                }
                Ty::DynTrait(b, _) => self.bounds(b, ctx),
                _ => {}
            }
        }
        fn pf_index(&self) -> u32 { self.prog.files.iter().position(|f| std::ptr::eq(&**f, self.pf)).unwrap_or(0) as u32 }
        fn bounds(&mut self, l: List<Bound>, ctx: V) {
            let ast = self.ast;
            for b in ast.list(l) {
                match *b {
                    Bound::Trait { path, .. } => self.path_inv(path, ctx),
                    Bound::Lifetime(lt) => { let r = self.nm.named(lt.lo, lt.hi); self.record(r, xform(ctx, V::Inv)); }
                    Bound::Use(_) => {}
                }
            }
        }
        /// all lifetimes and types in a path, invariantly
        fn path_inv(&mut self, p: Path, ctx: V) {
            let ast = self.ast;
            if let Some(q) = ast.qself(&p) { self.inv(q.ty, ctx); }
            for (i, seg) in ast.segs(p.segs).iter().enumerate() {
                let mut k = 0;
                while let Some(r) = self.nm.path(p.segs.lo + i as u32, k) { self.record(Some(r), xform(ctx, V::Inv)); k += 1; }
                match ast.gargs(seg.args()) {
                    Some(GenericArgs::Angle(l)) => for a in ast.list(*l) {
                        match *a {
                            GenericArg::Lifetime(lt) => { let r = self.nm.named(lt.lo, lt.hi); self.record(r, xform(ctx, V::Inv)); }
                            GenericArg::Type(t) | GenericArg::Binding(_, _, t) => self.inv(t, ctx),
                            GenericArg::Constraint(_, _, b) => self.bounds(b, ctx),
                            _ => {}
                        }
                    },
                    Some(GenericArgs::Paren(tys, ret)) => {
                        for &t in ast.list(*tys) { self.inv(t, ctx); }
                        if let Some(r) = ret { self.inv(*r, ctx); }
                    }
                    None => {}
                }
            }
        }
        fn path(&mut self, p: Path, ctx: V) {
            let ast = self.ast;
            // `Adt<..>` of a single segment resolves to its declaration's variances
            let segs = ast.segs(p.segs);
            if let ([seg], false) = (segs, p.has_qself()) {
                let name = self.prog.syms.intern_late(self.pf.text(seg.name));
                if let Some(a) = self.prog.lookup_in_scope(self.module, name, true) {
                    let adef = self.prog.def(a);
                    if matches!(adef.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) {
                        if let Some(adt) = self.tcx.adts.get(&a) {
                            let variances = self.tcx.variances_of(self.prog, a);
                            let idx = adt.generic_indices.clone();
                            let (mut li, mut ti) = (0usize, 0usize);
                            let mut k = 0;
                            // hidden lifetime arguments (`Foo` for `Foo<'a>`)
                            while let Some(r) = self.nm.path(p.segs.lo, k) {
                                let v = idx.lifetimes.get(li).and_then(|&f| variances.get(f as usize)).copied().unwrap_or(V::Inv);
                                self.record(Some(r), xform(ctx, v)); li += 1; k += 1;
                            }
                            if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                                for a in ast.list(*l) {
                                    match *a {
                                        GenericArg::Lifetime(lt) => {
                                            let v = idx.lifetimes.get(li).and_then(|&f| variances.get(f as usize)).copied().unwrap_or(V::Inv);
                                            let r = self.nm.named(lt.lo, lt.hi); self.record(r, xform(ctx, v)); li += 1;
                                        }
                                        GenericArg::Type(t) => {
                                            let v = idx.types_consts.get(ti).and_then(|&f| variances.get(f as usize)).copied().unwrap_or(V::Inv);
                                            self.walk(t, xform(ctx, v)); ti += 1;
                                        }
                                        GenericArg::Const(_) => ti += 1,
                                        _ => {}
                                    }
                                }
                            }
                            return;
                        }
                    }
                }
            }
            self.path_inv(p, ctx);
        }
    }
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let mut cx = Cx { prog, tcx, module: tcx.item_module(prog, d), ast, pf, nm, captured, out: HashMap::new() };
    if let ItemKind::Fn(sig, _, _) = ast.item(def.item).kind {
        let sig = *ast.fn_sig(sig);
        for p in ast.list(sig.params) { cx.walk(p.ty, V::Contra); }
        // the type `Self` stands for in an inherent or trait impl: its lifetimes occur in the receiver
        let mut self_ty: Option<(TyId, std::sync::Arc<LifetimeResolution>, DefId)> = None;
        if def.kind == DefKind::AssocFn {
            let parent = prog.def(def.parent);
            let past = &prog.files[parent.file as usize].ast;
            if let ItemKind::Impl(r) = past.item(parent.item).kind {
                self_ty = Some((past.row(r).self_ty, names(prog, tcx, def.parent), def.parent));
            }
        }
        let mut receiver: Vec<(TyId, V)> = Vec::new();
        match sig.self_param {
            Some(SelfParam::Ref(lt, m)) => {
                let r = match lt { Some(l) => nm.named(l.lo, l.hi), None => nm.self_reference() };
                cx.record(r, V::Contra);
                if let Some((t, _, _)) = &self_ty { receiver.push((*t, xform(V::Contra, if m { V::Inv } else { V::Co }))); }
            }
            Some(SelfParam::Typed(_, t)) => cx.walk(t, V::Contra),
            Some(SelfParam::Value(..)) => if let Some((t, _, _)) = &self_ty { receiver.push((*t, V::Contra)); },
            _ => {}
        }
        if let Some(r) = sig.ret { cx.walk(r, V::Co); }
        let mut out = std::mem::take(&mut cx.out);
        if let Some((_, pn, pd)) = &self_ty {
            let parent = prog.def(*pd);
            let ppf = &prog.files[parent.file as usize];
            let mut pcx = Cx { prog, tcx, module: tcx.item_module(prog, *pd), ast: &ppf.ast, pf: ppf, nm: pn, captured, out: HashMap::new() };
            for (t, v) in receiver { pcx.walk(t, v); }
            for (id, v) in pcx.out { let e = out.entry(id).or_insert(V::Bi); *e = join(*e, v); }
        }
        return out;
    }
    cx.out
}

/// rustc_lint impl_trait_overcaptures.rs `check_fn`: in an edition before 2024 an `impl Trait` in a
/// fn's return type captures only the lifetimes its bounds mention; from edition 2024 it captures
/// every one in scope. Warns for an opaque type (without `use<..>`) that leaves a lifetime
/// uncaptured, except a contravariant one that outlives a captured one.
fn impl_trait_overcaptures(prog: &Program, tcx: &Tcx, d: DefId) {
    use crate::tcx::resolve_bound_vars::ResolvedArg;
    use crate::types::variance::V;
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    if pf.edition >= 2024 { return; }
    let ast = &pf.ast;
    let item = *ast.item(def.item);
    let ItemKind::Fn(sig, _, _) = item.kind else { return };
    let sig = *ast.fn_sig(sig);
    let Some(ret) = sig.ret else { return };
    let mut opaques = Vec::new();
    opaque_types(ast, ret, &mut opaques);
    if opaques.is_empty() { return; }
    let nm = names(prog, tcx, d);
    let rbv = tcx.resolve_bound_vars(prog, d);
    // a captured lifetime of an opaque is a copy of a lifetime of the enclosing scope: map copies back
    let mut origin: HashMap<crate::types::LifetimeId, crate::types::LifetimeId> = HashMap::new();
    for caps in rbv.opaque_captured_lifetimes.values() {
        for &(arg, copy) in caps {
            if let ResolvedArg::EarlyBound(id) | ResolvedArg::LateBound(_, _, id) | ResolvedArg::Free(_, id) = arg { origin.insert(copy, id); }
        }
    }
    let root = |mut id: crate::types::LifetimeId| { let mut n = 0; while let Some(&o) = origin.get(&id) { id = o; n += 1; if n > 64 { break; } } id };
    let mut captured: HashMap<u32, Vec<crate::types::LifetimeId>> = HashMap::new();
    for (&opaque, caps) in &rbv.opaque_captured_lifetimes {
        let mut ids = Vec::new();
        for &(arg, _) in caps {
            if let ResolvedArg::EarlyBound(id) | ResolvedArg::LateBound(_, _, id) | ResolvedArg::Free(_, id) = arg { ids.push(root(id)); }
        }
        captured.insert(opaque.0, ids);
    }
    let in_scope = nm.in_scope_lifetimes();
    let variances = sig_lifetime_variances(prog, tcx, d, &nm, &captured);
    // the free region map: which in-scope lifetimes outlive which
    let edges = outlives_edges(prog, tcx, d, &nm);
    let mut all: Vec<LifetimeRes> = vec![LifetimeRes::Static];
    all.extend(in_scope.iter().map(|&id| LifetimeRes::Param(id)));
    let n = all.len();
    let mut reach = vec![vec![false; n]; n];
    for i in 0..n { reach[i][i] = true; reach[0][i] = true; }
    for (a, b) in edges {
        if let (Some(a), Some(b)) = (all.iter().position(|x| *x == a), all.iter().position(|x| *x == b)) { reach[a][b] = true; }
    }
    for k in 0..n { for i in 0..n { if reach[i][k] { for j in 0..n { if reach[k][j] { reach[i][j] = true; } } } } }
    let outlives = |a: crate::types::LifetimeId, b: crate::types::LifetimeId| {
        match (all.iter().position(|x| *x == LifetimeRes::Param(a)), all.iter().position(|x| *x == LifetimeRes::Param(b))) {
            (Some(a), Some(b)) => reach[a][b],
            _ => false,
        }
    };
    let w = W::new(prog, def.file, d);
    // walk the return type: each opaque, with the binders enclosing it
    struct Walk<'a, F: FnMut(TyId, &[crate::types::LifetimeId])> { ast: &'a Ast, nm: &'a LifetimeResolution, binders: Vec<crate::types::LifetimeId>, f: F }
    impl<F: FnMut(TyId, &[crate::types::LifetimeId])> Walk<'_, F> {
        fn bounds(&mut self, l: List<Bound>) {
            let ast = self.ast;
            for b in ast.list(l) {
                if let Bound::Trait { hrtb, path, .. } = *b {
                    let mark = self.binders.len();
                    for p in ast.list(hrtb) {
                        if let GenericParamKind::Lifetime(_) = p.kind {
                            if let Some(LifetimeRes::Param(id)) = self.nm.named(p.name.lo, p.name.hi).or_else(|| self.nm.declared_at(p.name.lo)) { self.binders.push(id); }
                        }
                    }
                    self.path(path);
                    self.binders.truncate(mark);
                }
            }
        }
        fn path(&mut self, p: Path) {
            let ast = self.ast;
            if let Some(q) = ast.qself(&p) { self.ty(q.ty); if let Some(tp) = q.trait_path { self.path(tp); } }
            for seg in ast.segs(p.segs) {
                match ast.gargs(seg.args()) {
                    Some(GenericArgs::Angle(l)) => for a in ast.list(*l) {
                        match *a {
                            GenericArg::Type(t) | GenericArg::Binding(_, _, t) => self.ty(t),
                            GenericArg::Constraint(_, _, b) => self.bounds(b),
                            _ => {}
                        }
                    },
                    Some(GenericArgs::Paren(tys, ret)) => {
                        for &t in ast.list(*tys) { self.ty(t); }
                        if let Some(r) = ret { self.ty(*r); }
                    }
                    None => {}
                }
            }
        }
        fn ty(&mut self, t: TyId) {
            let ast = self.ast;
            match *ast.ty(t) {
                Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => self.ty(x),
                Ty::Tuple(l) => for &x in ast.list(l) { self.ty(x); },
                Ty::Fn(f) => {
                    let f = *ast.fn_ptr(f);
                    for &x in ast.list(f.params) { self.ty(x); }
                    if let Some(r) = f.ret { self.ty(r); }
                }
                Ty::Path(p) => self.path(p),
                Ty::ImplTrait(b) => {
                    let binders = self.binders.clone();
                    (self.f)(t, &binders);
                    self.bounds(b);
                }
                Ty::DynTrait(b, _) => self.bounds(b),
                _ => {}
            }
        }
    }
    let mut found: Vec<(TyId, Vec<crate::types::LifetimeId>)> = Vec::new();
    {
        let mut wk = Walk { ast, nm: &nm, binders: Vec::new(), f: |t, b: &[crate::types::LifetimeId]| found.push((t, b.to_vec())) };
        wk.ty(ret);
    }
    for (t, binders) in found {
        let Ty::ImplTrait(b) = *ast.ty(t) else { continue };
        if ast.list(b).iter().any(|x| matches!(x, Bound::Use(_))) { continue; }
        let Some(&opaque) = prog.node_id_to_def_id.get(&(def.file, relax_front::def_collector::NodeId::OpaqueTy(t))) else { continue };
        let caps: &[crate::types::LifetimeId] = captured.get(&opaque.0).map_or(&[], |v| &v[..]);
        // the lifetimes in scope: the item's, and those of the binders around the opaque
        let mut uncaptured: Vec<(crate::types::LifetimeId, bool)> = in_scope.iter().filter(|id| !caps.contains(id)).map(|&id| (id, false)).collect();
        uncaptured.extend(binders.iter().filter(|id| !caps.contains(id)).map(|&id| (id, true)));
        // a contravariant lifetime that outlives a captured one need not be captured
        uncaptured.retain(|&(id, late)| {
            if late { return true; }
            if !matches!(variances.get(&id), Some(V::Bi | V::Contra)) { return true; }
            !caps.iter().any(|&r| outlives(id, r))
        });
        if uncaptured.is_empty() { continue; }
        let pos = w.ty_start(t).unwrap_or(item.lo);
        if lint::external(prog, def.file, pos) { continue; }
        emit(prog, d, &IMPL_TRAIT_OVERCAPTURES, def.file, pos, &"`impl Trait` will capture more lifetimes than possibly intended in edition 2024");
    }
}

// ---------------------------------------------------------------- redundant_lifetimes

pub const REDUNDANT_LIFETIMES: Lint = lint::lint("redundant_lifetimes");

/// The regions written in a type (references, explicit and hidden path arguments).
fn ty_regions(ast: &Ast, names: &LifetimeResolution, t: TyId, out: &mut Vec<LifetimeRes>) {
    match *ast.ty(t) {
        Ty::Ref(lt, _, x) => {
            let r = match lt { Some(l) => names.named(l.lo, l.hi), None => names.reference(t.0) };
            if let Some(r) = r { out.push(r); }
            ty_regions(ast, names, x, out);
        }
        Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => ty_regions(ast, names, x, out),
        Ty::Tuple(l) => for &x in ast.list(l) { ty_regions(ast, names, x, out); },
        Ty::Path(p) => {
            for (i, seg) in ast.segs(p.segs).iter().enumerate() {
                let mut k = 0;
                while let Some(r) = names.path(p.segs.lo + i as u32, k) { out.push(r); k += 1; }
                if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                    for a in ast.list(*l) {
                        match *a {
                            GenericArg::Lifetime(lt) => if let Some(r) = names.named(lt.lo, lt.hi) { out.push(r) },
                            GenericArg::Type(t) => ty_regions(ast, names, t, out),
                            _ => {}
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// What the well-formedness of `t` implies: for each `&'r X`, the regions of `X` outlive `'r`.
fn wf_edges(cx: &(&Program, u32, u32), names: &LifetimeResolution, t: TyId, edges: &mut Vec<(LifetimeRes, LifetimeRes)>) {
    let ast = &cx.0.files[cx.2 as usize].ast;
    match *ast.ty(t) {
        Ty::Ref(lt, _, x) => {
            let r = match lt { Some(l) => names.named(l.lo, l.hi), None => names.reference(t.0) };
            if let Some(r) = r {
                let mut inner = Vec::new();
                ty_regions(ast, names, x, &mut inner);
                for i in inner { edges.push((i, r)); }
            }
            wf_edges(cx, names, x, edges);
        }
        Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Array(x, _) | Ty::Paren(x) => wf_edges(cx, names, x, edges),
        Ty::Tuple(l) => for &x in ast.list(l) { wf_edges(cx, names, x, edges); },
        Ty::Path(p) => {
            // `Adt<X>` with `T: 'static` on the ADT's parameter: the regions of X outlive 'static
            let segs = ast.segs(p.segs);
            if let ([seg], false) = (segs, p.has_qself()) {
                let (prog, module, file) = *cx;
                if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                    let name = prog.syms.intern_late(prog.files[file as usize].text(seg.name));
                    if let Some(a) = prog.lookup_in_scope(module, name, true) {
                        let adef = prog.def(a);
                        if matches!(adef.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) {
                            let aast = &prog.files[adef.file as usize].ast;
                            let g = match aast.item(adef.item).kind {
                                ItemKind::Struct(r) => Some(aast.row(r).generics),
                                ItemKind::Enum(r) => Some(aast.row(r).generics),
                                ItemKind::Union(r) => Some(aast.row(r).generics),
                                _ => None,
                            };
                            if let Some(g) = g {
                                let targs: Vec<TyId> = ast.list(*l).iter().filter_map(|a| if let GenericArg::Type(t) = *a { Some(t) } else { None }).collect();
                                let tparams: Vec<&GenericParam> = aast.list(g.params).iter().filter(|p| matches!(p.kind, GenericParamKind::Type(..))).collect();
                                let apf = &prog.files[adef.file as usize];
                                for (t, p) in targs.iter().zip(tparams) {
                                    if let GenericParamKind::Type(bounds, _) = p.kind {
                                        if aast.list(bounds).iter().any(|b| matches!(*b, Bound::Lifetime(lt) if apf.text(lt) == "'static")) {
                                            let mut rs = Vec::new();
                                            ty_regions(ast, names, *t, &mut rs);
                                            for r in rs { edges.push((r, LifetimeRes::Static)); }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for seg in segs {
                if let Some(GenericArgs::Angle(l)) = ast.gargs(seg.args()) {
                    for a in ast.list(*l) { if let GenericArg::Type(t) = *a { wf_edges(cx, names, t, edges); } }
                }
            }
        }
        _ => {}
    }
}

/// The `'a: 'b` bounds of a generics list.
fn declared_edges(ast: &Ast, names: &LifetimeResolution, g: Generics, edges: &mut Vec<(LifetimeRes, LifetimeRes)>) {
    let mut add = |a: Ident, bounds: List<Ident>| {
        if let Some(a) = names.named(a.lo, a.hi) {
            for &b in ast.list(bounds) { if let Some(b) = names.named(b.lo, b.hi) { edges.push((a, b)); } }
        }
    };
    for p in ast.list(g.params) { if let GenericParamKind::Lifetime(bounds) = p.kind { add(p.name, bounds); } }
    for w in ast.list(g.where_) { if let WherePred::Lifetime(a, bounds) = *w { add(a, bounds); } }
}

/// The outlives relation the signature gives: the declared `'a: 'b` bounds of the item and its parents,
/// plus what the well-formedness of the signature's reference types implies (rustc's implied bounds,
/// `OutlivesEnvironment` free region map). `(a, b)`: `a` outlives `b`.
fn outlives_edges(prog: &Program, tcx: &Tcx, d: DefId, nm: &LifetimeResolution) -> Vec<(LifetimeRes, LifetimeRes)> {
    let def = prog.def(d);
    let pf = &prog.files[def.file as usize];
    let ast = &pf.ast;
    let mut edges: Vec<(LifetimeRes, LifetimeRes)> = Vec::new();
    let item = *ast.item(def.item);
    let mut impl_self: Option<(TyId, std::sync::Arc<LifetimeResolution>)> = None;
    if matches!(def.kind, DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst) {
        let parent = prog.def(def.parent);
        let past = &prog.files[parent.file as usize].ast;
        let pn = names(prog, tcx, def.parent);
        match past.item(parent.item).kind {
            ItemKind::Impl(r) => {
                let im = past.row(r);
                declared_edges(past, &pn, im.generics, &mut edges);
                wf_edges(&(prog, tcx.item_module(prog, def.parent), parent.file), &pn, im.self_ty, &mut edges);
                impl_self = Some((im.self_ty, pn));
            }
            ItemKind::Trait(r) => declared_edges(past, &pn, past.row(r).generics, &mut edges),
            _ => {}
        }
    }
    match item.kind {
        ItemKind::Fn(sig, _, _) => {
            let sig = *ast.fn_sig(sig);
            declared_edges(ast, nm, sig.generics, &mut edges);
            let cx = (prog, tcx.item_module(prog, d), def.file);
            for p in ast.list(sig.params) { wf_edges(&cx, nm, p.ty, &mut edges); }
            if let Some(r) = sig.ret { wf_edges(&cx, nm, r, &mut edges); }
            match sig.self_param {
                Some(SelfParam::Ref(lt, _)) => {
                    let r = match lt { Some(l) => nm.named(l.lo, l.hi), None => nm.self_reference() };
                    if let (Some(r), Some((st, pn))) = (r, &impl_self) {
                        let pa = &prog.files[prog.def(def.parent).file as usize].ast;
                        let mut inner = Vec::new();
                        ty_regions(pa, pn, *st, &mut inner);
                        for i in inner { edges.push((i, r)); }
                    }
                }
                Some(SelfParam::Typed(_, t)) => wf_edges(&cx, nm, t, &mut edges),
                _ => {}
            }
        }
        ItemKind::Struct(r) => declared_edges(ast, nm, ast.row(r).generics, &mut edges),
        ItemKind::Union(r) => declared_edges(ast, nm, ast.row(r).generics, &mut edges),
        ItemKind::Enum(r) => declared_edges(ast, nm, ast.row(r).generics, &mut edges),
        ItemKind::Trait(r) => declared_edges(ast, nm, ast.row(r).generics, &mut edges),
        ItemKind::Impl(r) => {
            let im = ast.row(r);
            declared_edges(ast, nm, im.generics, &mut edges);
            wf_edges(&(prog, tcx.item_module(prog, d), def.file), nm, im.self_ty, &mut edges);
        }
        _ => {}
    }
    edges
}

/// rustc_hir_analysis wfcheck `lint_redundant_lifetimes`: a lifetime parameter of the item that
/// provably equals an earlier one (or `'static`). The outlives relation is the declared bounds
/// plus what the well-formedness of the signature's reference types implies; where the walk
/// cannot follow a type it adds no fact, so it can only report fewer lifetimes than rustc.
fn redundant_lifetimes(prog: &Program, tcx: &Tcx, d: DefId) {
    let def = prog.def(d);
    match def.kind {
        DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Trait | DefKind::Fn | DefKind::Const | DefKind::Impl => {}
        DefKind::AssocFn | DefKind::AssocTy | DefKind::AssocConst => {
            let p = prog.def(def.parent);
            if p.kind == DefKind::Impl && tcx.impl_trait.contains_key(&def.parent) { return; }
        }
        _ => return,
    }
    let nm = names(prog, tcx, d);
    let scope = nm.named_in_scope();
    if !scope.iter().any(|s| s.3) { return; }
    let edges = outlives_edges(prog, tcx, d, &nm);
    // the candidates: 'static, then the named lifetimes in scope
    let mut all: Vec<LifetimeRes> = vec![LifetimeRes::Static];
    all.extend(scope.iter().map(|s| LifetimeRes::Param(s.0)));
    let n = all.len();
    let idx = |r: LifetimeRes| all.iter().position(|x| *x == r);
    let mut reach = vec![vec![false; n]; n];
    for i in 0..n { reach[i][i] = true; reach[0][i] = true; }
    for (a, b) in edges {
        if let (Some(a), Some(b)) = (idx(a), idx(b)) { reach[a][b] = true; }
    }
    for k in 0..n { for i in 0..n { if reach[i][k] { for j in 0..n { if reach[k][j] { reach[i][j] = true; } } } } }
    let mut shadowed = vec![false; n];
    for c in 0..n {
        if shadowed[c] { continue; }
        for v in (c + 1)..n {
            let (id, ref name, span, own) = scope[v - 1];
            let _ = id;
            if !own || !(reach[c][v] && reach[v][c]) { continue; }
            shadowed[v] = true;
            if lint::external(prog, def.file, span) { continue; }
            emit(prog, d, &REDUNDANT_LIFETIMES, def.file, span, &format!("unnecessary lifetime parameter `{}`", name));
        }
    }
}

// ---------------------------------------------------------------- entry points

/// The definition-level passes of this file over one definition.
pub fn check_def_item(prog: &Program, tcx: &Tcx, d: DefId) {
    match prog.def(d).kind {
        DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => { mismatched_lifetime_syntaxes(prog, tcx, d); impl_trait_redundant_captures(prog, tcx, d); impl_trait_overcaptures(prog, tcx, d); redundant_lifetimes(prog, tcx, d); }
        DefKind::Struct | DefKind::Union | DefKind::Enum => { explicit_outlives_requirements(prog, d); redundant_lifetimes(prog, tcx, d); }
        DefKind::Trait | DefKind::Impl | DefKind::Const => redundant_lifetimes(prog, tcx, d),
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
pub(crate) fn string_value(t: &[u8]) -> Option<String> {
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

pub const INVALID_DOC_ATTRIBUTES: Lint = lint::lint("invalid_doc_attributes");
pub const UNKNOWN_DIAGNOSTIC_ATTRIBUTES: Lint = lint::lint("unknown_diagnostic_attributes");
/// A malformed attribute that rustc rejects outright (no lint of its own): never allowed.

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
    // the `doc(alias)` values of the node, across its doc attributes
    let doc_aliases: std::cell::RefCell<Vec<String>> = Default::default();
    for &(file, a) in n.attrs {
        let pf = &prog.files[file as usize];
        let apos = attr_pos(pf, &a);
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        let metas = parse_meta_items(pf, lo, hi);
        let Some(Meta::Item { path, args, .. }) = metas.first() else { continue };
        let emit = |lint: &Lint, pos: u32, msg: &str| {
            if !lint::external(prog, file, pos) { emit_in(prog, d, n.inner, lint, file, pos, msg); }
        };
        let hard = |pos: u32, msg: &str| {
            if !lint::external(prog, file, pos) { lint::hard_error(prog, file, pf.lint_pos(pos), msg); }
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
                                if !ok { hard(apos, "E0539: malformed `macro_use` attribute input"); }
                            }
                        }
                    }
                    Args::Value { .. } => hard(apos, "E0539: malformed `macro_use` attribute input"),
                }
            }
        }
        // ---- doc attributes
        if word.as_deref() == Some("doc") {
            doc_attr(prog, n, file, apos, args, &emit, &hard, &doc_aliases);
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
fn doc_attr(prog: &Program, n: &AttrNode, file: u32, apos: u32, args: &Args, emit: &dyn Fn(&Lint, u32, &str), hard: &dyn Fn(u32, &str), aliases_seen: &std::cell::RefCell<Vec<String>>) {
    let _ = prog;
    let invalid = |pos: u32, msg: &str| emit(&INVALID_DOC_ATTRIBUTES, pos, msg);
    match args {
        Args::None => {}
        Args::Value { pos, tok } => {
            // (`doc = include_str!(..)` / `concat!(..)`: a macro call, expanded to a string literal before the check)
            let literal_prefix = tok.starts_with(b"r\"") || tok.starts_with(b"r#") || tok.starts_with(b"b\"") || tok.starts_with(b"b'") || tok.starts_with(b"br");
            let macro_call = tok.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') && !literal_prefix;
            if !macro_call && string_value(tok).is_none() { invalid(*pos, "malformed `doc` attribute input"); }
        }
        Args::List { items, .. } => {
            let add_alias = |alias: String, pos: u32| {
                if alias.is_empty() { hard(pos, "`#[doc(alias = \"...\")]` attribute cannot have empty value"); return; }
                if let Some(c) = alias.chars().find(|&c| c == '"' || c == '\'' || (c.is_whitespace() && c != ' ')) {
                    hard(pos, &format!("{:?} character isn't allowed in `#[doc(alias = \"...\")]`", c));
                    return;
                }
                if alias.starts_with(' ') || alias.ends_with(' ') { hard(pos, "`#[doc(alias = \"...\")]` cannot start or end with ' '"); return; }
                if n.is_crate { hard(pos, "`#![doc(alias = \"...\")]` isn't allowed as a crate-level attribute"); return; }
                // check_attr.rs check_doc_alias_value: an alias is not the item's own name
                if n.inner.is_empty() && prog.syms.str(prog.def(n.d).name) == alias { hard(pos, &format!("`#[doc(alias = \"{}\"]` is the same as the item's name", alias)); return; }
                if aliases_seen.borrow().contains(&alias) { emit(&UNUSED_ATTRIBUTES, pos, "doc alias is duplicated"); }
                aliases_seen.borrow_mut().push(alias);
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
            if matches!(item.kind, ItemKind::Use(_)) { absolute_use_paths(prog, d, file, item, &own_attrs); unused_import_braces(prog, d, file, item, &own_attrs); }
            if attrs.is_empty() { continue; }
            attr_checks(prog, &AttrNode { d, inner: &own_attrs, attrs: &attrs, is_crate: false, macro_use_target: matches!(item.kind, ItemKind::ExternCrate(_)) });
        }
    }
}

// ---------------------------------------------------------------- explicit_outlives_requirements

pub const EXPLICIT_OUTLIVES_REQUIREMENTS: Lint = lint::lint("explicit_outlives_requirements");

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

// ---------------------------------------------------------------- crate attributes

pub const UNKNOWN_CRATE_TYPES: Lint = lint::lint("unknown_crate_types");
pub const MISSING_DOCS: Lint = lint::lint("missing_docs");

/// `#[cfg_attr(pred,)]` (the parser records it under `makepad check`): rustc_expand config.rs.
fn cfg_attr_empties(prog: &Program, own: &[bool]) {
    for (path, pos) in crate::macros::uses::take_cfg_attr_empty() {
        let Some(file) = prog.file_paths.iter().position(|p| *p == path) else { continue };
        let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        let def = enclosing_def(prog, file as u32, pos, krate);
        emit(prog, def, &UNUSED_ATTRIBUTES, file as u32, pos, "`#[cfg_attr]` does not expand to any attributes");
    }
}

pub const NAMED_ARGUMENTS_USED_POSITIONALLY: Lint = lint::lint("named_arguments_used_positionally");

fn named_args_positional(prog: &Program, own: &[bool]) {
    for (path, pos, name) in crate::macros::uses::take_named_arg_positional() {
        let Some(file) = prog.file_paths.iter().position(|p| *p == path) else { continue };
        let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        if lint::external(prog, file as u32, pos) { continue; }
        let def = enclosing_def(prog, file as u32, prog.files[file].lint_pos(pos), krate);
        emit(prog, def, &NAMED_ARGUMENTS_USED_POSITIONALLY, file as u32, pos, &format!("named argument `{}` is not used by name", name));
    }
}

/// The crate-level attribute lints: `unknown_crate_types` (an invalid `#![crate_type]`) and the
/// crate's `missing_docs`.
fn crate_attr_lints(prog: &Program, own: &[bool]) {
    const VALID: &[&str] = &["bin", "lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "sdylib"];
    for md in prog.mods.iter() {
        if md.parent_mod.is_some() || md.lexical_parent.is_some() || !own.get(md.krate as usize).copied().unwrap_or(false) { continue; }
        let pf = &prog.files[md.file as usize];
        let ast = &pf.ast;
        let attrs: Vec<Attr> = ast.root_attrs.iter().copied().collect();
        let root = md.def;
        for a in &attrs {
            if lint::attr_path(prog, md.file, a) != b"crate_type" { continue; }
            let text = lint::attr_text(prog, md.file, a);
            let Some(v) = text.split_once('=').map(|x| x.1.trim().trim_matches('"').to_string()) else { continue };
            if !VALID.contains(&v.as_str()) {
                let pos = prog.src(md.file).tok(a.toks.lo as usize).lo;
                if !lint::external(prog, md.file, pos) { emit(prog, root, &UNKNOWN_CRATE_TYPES, md.file, pos, "invalid `crate_type` value"); }
            }
        }
        // missing_docs for the crate: no `#![doc]` / `//!`
        let first_item = ast.root_items.iter().next().map_or(pf.src_len, |&i| ast.item(i).lo);
        let head = pf.bytes(0, first_item.min(pf.src_len));
        let doc_comment = head.split(|&b| b == b'\n').any(|l| { let t = l.trim_ascii_start(); t.starts_with(b"//!") || t.starts_with(b"/*!") });
        let doc_attr = attrs.iter().any(|a| lint::attr_path(prog, md.file, a) == b"doc" && lint::attr_text(prog, md.file, a).contains('='));
        if !doc_comment && !doc_attr {
            let pos = attrs.first().map(|a| prog.src(md.file).tok(a.toks.lo as usize).lo).or_else(|| ast.root_items.iter().next().map(|&i| ast.item(i).lo)).unwrap_or(0);
            emit(prog, root, &MISSING_DOCS, md.file, pos, "missing documentation for the crate");
        }
    }
}

// ---------------------------------------------------------------- semicolon_in_expressions_from_macros

pub const SEMICOLON_IN_EXPRESSIONS_FROM_MACROS: Lint = lint::lint("semicolon_in_expressions_from_macros");

/// rustc_expand ParserAnyMacro::make: a local `macro_rules!` expansion parsed as an expression
/// that ends with a `;`. The parser records the `;` (only under `makepad check`).
fn trailing_semis(prog: &Program, own: &[bool]) {
    let mut seen = std::collections::HashSet::new();
    for t in crate::macros::uses::take_trailing_semis() {
        let Some(file) = prog.file_paths.iter().position(|p| *p == t.file) else { continue };
        let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
        if !own.get(krate as usize).copied().unwrap_or(false) { continue; }
        // `is_local`: the macro is defined in the crate that calls it
        let local = prog.file_paths.iter().position(|p| *p == t.def_file).is_some_and(|f| prog.file_crate.get(f).copied() == Some(krate));
        if !local { continue; }
        if !seen.insert((file, t.semi, t.call)) { continue; }
        let pf = &prog.files[file];
        let def = enclosing_def(prog, file as u32, pf.lint_pos(t.call), krate);
        let attrs = lint::attrs_at_call(prog, def, file as u32, t.call);
        emit_in(prog, def, &attrs, &SEMICOLON_IN_EXPRESSIONS_FROM_MACROS, file as u32, t.semi, "trailing semicolon in macro used in expression position");
    }
}

// ---------------------------------------------------------------- meta_variable_misuse

pub const META_VARIABLE_MISUSE: Lint = lint::lint("meta_variable_misuse");

/// rustc_expand `check_meta_variables`, for every `macro_rules!` of the checked crates.
fn meta_variable_misuse(prog: &Program, own: &[bool]) {
    let mut seen = std::collections::HashSet::new();
    for (_, defs) in prog.macros.defs.iter() {
        for d in defs {
            if d.builtin != 0 || d.def_file.is_empty() { continue; }
            let Some(file) = prog.file_paths.iter().position(|p| *p == d.def_file) else { continue };
            let krate = prog.file_crate.get(file).copied().unwrap_or(u32::MAX);
            if !own.get(krate as usize).copied().unwrap_or(false) || d.def_pos >= prog.files[file].src_len || !seen.insert((file, d.def_pos)) { continue; }
            let Some(found) = relax_front::macro_check::check_meta_variables(d) else { continue };
            for f in found {
                if f.pos >= prog.files[file].src_len { continue; }
                let def = enclosing_def(prog, file as u32, f.pos, krate);
                emit(prog, def, &META_VARIABLE_MISUSE, file as u32, f.pos, &f.msg);
            }
        }
    }
}

// ---------------------------------------------------------------- unused_macros / unused_macro_rules

pub const UNUSED_MACROS: Lint = lint::lint("unused_macros");
pub const UNUSED_MACRO_RULES: Lint = lint::lint("unused_macro_rules");

/// rustc_resolve `check_unused_macros`, after the checked crates' bodies are all expanded: a
/// private `macro_rules!` no invocation resolved to, and the rules of the others that no
/// expansion matched.
pub fn finish(prog: &Program, own: &[bool]) {
    if !lint::enabled() { return; }
    trailing_semis(prog, own);
    meta_variable_misuse(prog, own);
    cfg_attr_empties(prog, own);
    named_args_positional(prog, own);
    crate_attr_lints(prog, own);
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
        }).unwrap_or_else(|| enclosing_def(prog, file as u32, d.pos, krate));
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

pub const ABSOLUTE_PATHS_NOT_STARTING_WITH_CRATE: Lint = lint::lint("absolute_paths_not_starting_with_crate");
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

/// rustc `UnusedImportBraces`: `use a::{b};` (a single non-`self` item in braces).
pub(crate) fn unused_import_braces(prog: &Program, d: DefId, file: u32, item: &Item, attrs: &[Attr]) {
    let pf = &prog.files[file as usize];
    let ast = &pf.ast;
    let ItemKind::Use(r) = item.kind else { return };
    fn walk(prog: &Program, d: DefId, file: u32, ast: &Ast, t: UseTree, item: &Item, attrs: &[Attr]) {
        match t {
            UseTree::Path(_, c) => walk(prog, d, file, ast, *ast.use_tree(c), item, attrs),
            UseTree::Group(l) => {
                let items = ast.list(l);
                for c in items { walk(prog, d, file, ast, *c, item, attrs); }
                let [one] = items else { return };
                let pf = &prog.files[file as usize];
                let node = match *one {
                    UseTree::Name(seg, rename) => {
                        if seg.kind() == SegKind::SelfValue { return; }
                        pf.text(rename.unwrap_or(seg.name)).to_string()
                    }
                    UseTree::Glob => "*".to_string(),
                    _ => return,
                };
                if lint::external(prog, file, item.lo) { return; }
                emit_in(prog, d, attrs, &UNUSED_IMPORT_BRACES, file, item.lo, &format!("braces around {} is unnecessary", node));
            }
            _ => {}
        }
    }
    walk(prog, d, file, ast, ast.row(r).tree, item, attrs);
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
    let root = crate_root_def(prog, krate);
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

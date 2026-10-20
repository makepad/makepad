//! Lint levels and the lint sink of `makepad check` (rustc_lint levels.rs, rustc_lint_defs).
//!
//! A lint's level at a definition is decided by the `allow` / `warn` / `deny` / `forbid` /
//! `expect` attributes on the definition and its enclosing items, modules and crate root (the
//! innermost attribute naming the lint or one of its groups wins; a `forbid` cannot be lowered
//! further in); `warnings` then re-levels whatever is still at Warn. A lint at Deny or Forbid is
//! an error of the check, at Warn a printed warning, at Allow nothing.
//!
//! Lints are only collected while the check enables the sink (`enable`): a run reports none.

use crate::ast::Attr;
use crate::program::{DefId, DefKind, Program, NO_DEF};
use relax_session::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level { Allow, Warn, Deny, Forbid }

pub struct Lint {
    pub name: &'static str,
    pub default: Level,
    /// the default level in edition 2024 code (rustc `@edition Edition2024 => level`)
    pub default_2024: Level,
    /// the lint groups it belongs to (besides `warnings`)
    pub groups: &'static [&'static str],
}

const fn level_of(k: crate::lint_table::LevelK) -> Level {
    match k {
        crate::lint_table::LevelK::Allow => Level::Allow,
        crate::lint_table::LevelK::Warn => Level::Warn,
        crate::lint_table::LevelK::Deny => Level::Deny,
        crate::lint_table::LevelK::Forbid => Level::Forbid,
    }
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() { return false; }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] { return false; }
        i += 1;
    }
    true
}

/// The lint `name` of the generated rustc table (lint_table.rs): its default level, its level
/// from edition 2024 on and its groups are rustc's. A name the table lacks does not compile.
pub const fn lint(name: &'static str) -> Lint {
    let mut i = 0;
    while i < crate::lint_table::LINTS.len() {
        let l = &crate::lint_table::LINTS[i];
        if same(l.0, name) {
            let default = level_of(l.1);
            let default_2024 = match l.2 {
                Some((ed, k)) if ed <= 2024 => level_of(k),
                _ => default,
            };
            return Lint { name, default, default_2024, groups: l.3 };
        }
        i += 1;
    }
    panic!("lint missing from lint_table.rs")
}

pub const UNUSED_MUT: Lint = lint("unused_mut");
pub const OVERFLOWING_LITERALS: Lint = lint("overflowing_literals");
pub const UNUSED_UNSAFE: Lint = lint("unused_unsafe");
pub const UNSAFE_OP_IN_UNSAFE_FN: Lint = lint("unsafe_op_in_unsafe_fn");
pub const STATIC_MUT_REFS: Lint = lint("static_mut_refs");
pub const UNREACHABLE_PATTERNS: Lint = lint("unreachable_patterns");
pub const UNCOVERED_PARAM_IN_PROJECTION: Lint = lint("uncovered_param_in_projection");
pub const COHERENCE_LEAK_CHECK: Lint = lint("coherence_leak_check");
pub const TYVAR_BEHIND_RAW_POINTER: Lint = lint("tyvar_behind_raw_pointer");
pub const AMBIGUOUS_ASSOCIATED_ITEMS: Lint = lint("ambiguous_associated_items");
pub const LATE_BOUND_LIFETIME_ARGUMENTS: Lint = lint("late_bound_lifetime_arguments");
pub const OVERLAPPING_RANGE_ENDPOINTS: Lint = lint("overlapping_range_endpoints");
pub const NON_CONTIGUOUS_RANGE_ENDPOINTS: Lint = lint("non_contiguous_range_endpoints");
pub const IRREFUTABLE_LET_PATTERNS: Lint = lint("irrefutable_let_patterns");
pub const BINDINGS_WITH_VARIANT_NAME: Lint = lint("bindings_with_variant_name");
pub const LONG_RUNNING_CONST_EVAL: Lint = lint("long_running_const_eval");
pub const ELIDED_LIFETIMES_IN_ASSOCIATED_CONSTANT: Lint = lint("elided_lifetimes_in_associated_constant");
pub const UNSAFE_CODE: Lint = lint("unsafe_code");
pub const MISSING_UNSAFE_ON_EXTERN: Lint = lint("missing_unsafe_on_extern");
pub const DEPRECATED_SAFE_2024: Lint = lint("deprecated_safe_2024");
pub const UNSAFE_ATTR_OUTSIDE_UNSAFE: Lint = lint("unsafe_attr_outside_unsafe");

const LEVEL_ATTRS: &[(&[u8], Level, bool)] = &[
    (b"allow", Level::Allow, false), (b"expect", Level::Allow, true), (b"warn", Level::Warn, false),
    (b"deny", Level::Deny, false), (b"forbid", Level::Forbid, false),
];

/// A lint level attribute: its level, whether it only expects, the names it lists with the
/// position of each, and its identity.
pub struct LevelAttr {
    pub level: Level,
    pub expect: bool,
    /// (name as written, position); tool lints keep their `::` path
    pub names: Vec<(String, u32)>,
    pub key: (u32, u32),
    pub reason: bool,
}

/// Identifies one name of one level attribute: (file, attribute token, name index).
pub type AttrKey = (u32, u32, u32);

pub fn parse_level_attr(prog: &Program, file: u32, a: &Attr) -> Option<LevelAttr> {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    if hi <= lo + 2 { return None; }
    let head = crate::cfg::tok_str(p, lo);
    let (level, expect) = LEVEL_ATTRS.iter().find(|(n, _, _)| *n == head).map(|x| (x.1, x.2))?;
    if crate::cfg::tok_str(p, lo + 1) != b"(" { return None; }
    let mut names: Vec<(String, u32)> = Vec::new();
    let mut cur = String::new();
    let mut start = 0u32;
    let mut reason = false;
    let mut i = lo + 2;
    let mut skip_value = false;
    let mut depth = 0u32;
    while i < hi {
        let t = crate::cfg::tok_str(p, i);
        match t {
            b"(" | b"[" | b"{" => { depth += 1; cur.clear(); skip_value = true; }
            b")" | b"]" | b"}" if depth > 0 => depth -= 1,
            b"," if depth > 0 => {}
            b")" | b"," => {
                if !cur.is_empty() && !skip_value { names.push((std::mem::take(&mut cur), start)); }
                cur.clear();
                skip_value = false;
                if t == b")" { break; }
            }
            b"=" => {
                // `reason = "..."`
                if cur == "reason" { reason = true; }
                cur.clear();
                skip_value = true;
            }
            b"::" => cur.push_str("::"),
            _ => {
                if cur.is_empty() { start = p.tok(i).lo; }
                // a literal is no lint name (E0452, reported by lint_levels)
                if cur.is_empty() && !t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') { skip_value = true; }
                if !skip_value { cur.push_str(&String::from_utf8_lossy(t)); }
            }
        }
        i += 1;
    }
    Some(LevelAttr { level, expect, names, key: (file, a.toks.lo), reason })
}

/// The groups a lint belongs to: the table's (rustc) and the ones its definition names.
fn groups_of(lint: &Lint) -> impl Iterator<Item = &'static str> {
    let table = crate::lint_table::LINTS.iter().find(|l| l.0 == lint.name).map(|l| l.3).unwrap_or(&[]);
    table.iter().copied().chain(lint.groups.iter().copied())
}

/// The name a lint is known by now (`bare_trait_object` was renamed `bare_trait_objects`).
pub fn current_name(n: &str) -> &str {
    if n == "bad_style" { return "nonstandard_style"; }
    crate::lint_table::RENAMED.iter().find(|(old, _)| *old == n).map_or(n, |(_, new)| new)
}

/// The attribute lists that apply at `def`, innermost first: the definition's own, then each
/// enclosing item's, module's (its `mod` item and its file's inner attributes) and the crate
/// root's inner attributes.
fn scopes(prog: &Program, def: DefId) -> Vec<(u32, Vec<Attr>)> {
    let mut out = Vec::new();
    let mut d = def;
    let mut guard = 0;
    while d != NO_DEF && guard < 256 {
        guard += 1;
        let dd = prog.def(d);
        let ast = &prog.files[dd.file as usize].ast;
        if dd.kind == DefKind::Mod {
            let m = &prog.mods[dd.sub as usize];
            let block = prog.block_owner.get(&dd.sub).is_some();
            if !block && dd.parent != NO_DEF {
                out.push((dd.file, ast.list(ast.item(dd.item).attrs).to_vec()));
            }
            // the module's own file (out-of-line module, crate root): its inner attributes
            if !block && (dd.parent == NO_DEF || m.file != dd.file) {
                let f = &prog.files[m.file as usize].ast;
                out.push((m.file, f.root_attrs.iter().copied().collect()));
            }
        } else if !matches!(dd.kind, DefKind::AnonConst | DefKind::Closure | DefKind::OpaqueTy | DefKind::Prim(_)) {
            out.push((dd.file, ast.list(ast.item(dd.item).attrs).to_vec()));
            // an item declared in a body: the attributes of the statements around it
            if dd.parent != NO_DEF {
                let mut owner = dd.parent;
                if prog.def(owner).kind == DefKind::Mod {
                    owner = prog.block_owner.get(&prog.def(owner).sub).copied().unwrap_or(NO_DEF);
                }
                while owner != NO_DEF && matches!(prog.def(owner).kind, DefKind::Closure | DefKind::AnonConst) { owner = prog.def(owner).parent; }
                if owner != NO_DEF && matches!(prog.def(owner).kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) {
                    let a = stmt_attrs_around(prog, owner, dd.file, dd.item);
                    if !a.is_empty() { out.push((dd.file, a)); }
                }
            }
        }
        d = dd.parent;
    }
    out
}

/// The level of `lint` at `def`.
pub fn level(prog: &Program, def: DefId, lint: &Lint) -> Level {
    level_in(prog, def, &[], lint)
}

/// The level of `lint` at `def` under the attributes `inner` of a node inside it.
pub fn level_with(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint) -> Level {
    level_in(prog, def, inner, lint)
}

/// The level of `lint` inside `def` under `inner`, the attributes of a node of its body (a
/// match arm) in the definition's file.
fn level_in(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint) -> Level {
    resolve(prog, def, inner, lint, None).level
}

/// A lint's level at a node, and where it came from.
pub struct Resolved {
    pub level: Level,
    /// the `expect` attribute (name) that put the lint at this level
    pub expect: Option<AttrKey>,
    /// the level was set by `forbid` naming a group, or on the command line
    pub forbid_from_group: bool,
    /// the attribute name that last set the level, as written
    pub source: Option<String>,
}

/// rustc_lint levels.rs `LintLevelsBuilder`: the level of `lint` at `def` inside `inner`, every
/// attribute from the crate root inwards applied in order. `until` stops before that attribute
/// (the state an attribute sees).
pub fn resolve(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint, until: Option<(u32, u32)>) -> Resolved {
    resolve_in(prog, def, def_file(prog, def), inner, lint, until)
}

fn def_file(prog: &Program, def: DefId) -> u32 { if def == NO_DEF { 0 } else { prog.def(def).file } }

/// `resolve` with the file the `inner` attributes are written in (a module's own `use` items
/// are in the module's file, not the file that declares the module).
pub fn resolve_in(prog: &Program, def: DefId, inner_file: u32, inner: &[Attr], lint: &Lint, until: Option<(u32, u32)>) -> Resolved {
    let mut scopes = scopes(prog, def);
    if !inner.is_empty() && def != NO_DEF {
        scopes.insert(0, (inner_file, inner.to_vec()));
    }
    let edition = prog.files[prog.def(def).file as usize].edition;
    let mut spec = Resolved { level: if edition >= 2024 { lint.default_2024 } else { lint.default }, expect: None, forbid_from_group: false, source: None };
    let mut warnings: Option<Level> = None;
    let mut forbid_warnings = false;
    let in_groups: Vec<&str> = groups_of(lint).collect();
    let matches = |n: &str| n == lint.name || in_groups.contains(&n);
    // rustc `insert_spec`
    let insert = |spec: &mut Resolved, level: Level, expect: Option<AttrKey>, is_group: bool, name: &str| {
        // a deny inside a forbid is ignored; any other lowering of a forbid is an error that
        // leaves it (or a future-compatibility warning for a forbidden group, which applies)
        if level == Level::Deny && spec.level == Level::Forbid { return; }
        // the lint cannot be expected: it would silence itself
        if expect.is_some() && lint.name == "unfulfilled_lint_expectations" { return; }
        if level != Level::Forbid && spec.level == Level::Forbid && !spec.forbid_from_group { return; }
        spec.level = level;
        spec.expect = expect;
        spec.forbid_from_group = level == Level::Forbid && is_group;
        spec.source = Some(name.to_string());
    };
    // the command line's levels (`-A unused`, `-D warnings`) come below every attribute
    for (l, n) in cmdline_levels() {
        let n = current_name(n);
        if matches(n) {
            insert(&mut spec, *l, None, n != lint.name, n);
        } else if n == "warnings" && !forbid_warnings {
            warnings = Some(*l);
            forbid_warnings = *l == Level::Forbid;
        }
    }
    'outer: for (file, attrs) in scopes.iter().rev() {
        for a in attrs {
            if until == Some((*file, a.toks.lo)) { break 'outer; }
            let Some(la) = parse_level_attr(prog, *file, a) else { continue };
            for (idx, (name, _)) in la.names.iter().enumerate() {
                let n = current_name(name);
                if matches(n) {
                    let key = if la.expect { Some((*file, a.toks.lo, idx as u32)) } else { None };
                    insert(&mut spec, la.level, key, n != lint.name, n);
                } else if n == "warnings" && !forbid_warnings {
                    warnings = Some(la.level);
                    forbid_warnings = la.level == Level::Forbid;
                }
            }
        }
    }
    // `-D warnings` / `forbid(warnings)` raise the lints that would only warn (but for the few
    // that opt out)
    if let (Level::Warn, Some(w)) = (spec.level, warnings) {
        if !matches!(lint.name, "forbidden_lint_groups" | "linker_messages") {
            spec.level = w;
            spec.forbid_from_group = w == Level::Forbid;
        }
    }
    spec
}

static FULFILLED: Mutex<Vec<AttrKey>> = Mutex::new(Vec::new());

/// The expectations (`#[expect(..)]` names) a lint was reported under.
pub fn fulfilled_expectations() -> Vec<AttrKey> {
    std::mem::take(&mut *FULFILLED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The lint levels of the command line, from `MAKEPAD_RUN_LINT_FLAGS` (rustc's `-A/-W/-D/-F`
/// flags, e.g. `-A unused -D warnings`), in order. The UI test harness passes `-A unused`, as
/// rustc's compiletest does.
fn cmdline_levels() -> &'static [(Level, String)] {
    static FLAGS: std::sync::OnceLock<Vec<(Level, String)>> = std::sync::OnceLock::new();
    FLAGS.get_or_init(|| {
        let Ok(v) = std::env::var("MAKEPAD_RUN_LINT_FLAGS") else { return Vec::new() };
        let mut out = Vec::new();
        let mut it = v.split_whitespace();
        while let Some(w) = it.next() {
            let (level, rest) = match w {
                "-A" | "--allow" => (Level::Allow, None),
                "-W" | "--warn" => (Level::Warn, None),
                "-D" | "--deny" => (Level::Deny, None),
                "-F" | "--forbid" => (Level::Forbid, None),
                _ if w.starts_with("-A") => (Level::Allow, Some(&w[2..])),
                _ if w.starts_with("-W") => (Level::Warn, Some(&w[2..])),
                _ if w.starts_with("-D") => (Level::Deny, Some(&w[2..])),
                _ if w.starts_with("-F") => (Level::Forbid, Some(&w[2..])),
                _ => continue,
            };
            if let Some(name) = rest.map(str::to_string).or_else(|| it.next().map(str::to_string)) {
                out.push((level, name));
            }
        }
        out
    })
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static SINK: Mutex<Vec<(Level, String)>> = Mutex::new(Vec::new());

/// `makepad check` collects lints (nothing else does).
pub fn enable() { ENABLED.store(true, Ordering::Relaxed); }
pub fn enabled() -> bool { ENABLED.load(Ordering::Relaxed) }

/// Reports `lint` at `pos` of `file` in `def`'s scope: "path:line:col: error|warning: msg".
pub fn emit(prog: &Program, def: DefId, lint: &Lint, file: u32, pos: u32, msg: &str) {
    emit_in(prog, def, &[], lint, file, pos, msg)
}

/// A hard error of the check that is no lint (rustc's attribute parsing errors, `E0539` ..): no
/// level applies to it.
pub fn hard_error(prog: &Program, file: u32, pos: u32, msg: &str) {
    if !enabled() { return; }
    let (l, c) = prog.files[file as usize].line_col(pos);
    let line = format!("{}:{}:{}: error: {}", prog.file_paths[file as usize], l, c, msg);
    let _k = crate::zone_alloc::kept();
    SINK.lock().unwrap_or_else(|e| e.into_inner()).push((Level::Deny, line));
}

/// `emit` under the attributes `inner` of a node inside `def`'s body.
pub fn emit_in(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint, file: u32, pos: u32, msg: &str) {
    if !enabled() { return; }
    let spec = resolve_in(prog, def, file, inner, lint, None);
    let level = spec.level;
    if level == Level::Allow {
        if let Some(k) = spec.expect { FULFILLED.lock().unwrap_or_else(|e| e.into_inner()).push(k); }
        return;
    }
    let (l, c) = prog.files[file as usize].line_col(pos);
    let kind = if level >= Level::Deny { "error" } else { "warning" };
    let line = format!("{}:{}:{}: {}: {} [{}]", prog.file_paths[file as usize], l, c, kind, msg, lint.name);
    let _k = crate::zone_alloc::kept();
    SINK.lock().unwrap_or_else(|e| e.into_inner()).push((level, line));
}

/// The lints reported since the last take: (errors, warnings).
pub fn take() -> (Vec<String>, Vec<String>) {
    let all = std::mem::take(&mut *SINK.lock().unwrap_or_else(|e| e.into_inner()));
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    // (a diagnostic reported twice -- a body checked again by the second borrowck -- is one)
    let mut seen = std::collections::HashSet::new();
    for (l, s) in all {
        if !seen.insert(s.clone()) { continue; }
        if l >= Level::Deny { errors.push(s) } else { warnings.push(s) }
    }
    (errors, warnings)
}

// ---------------------------------------------------------------- overflowing_literals

/// rustc_lint types/literal.rs: an integer literal whose value does not fit its type (a negated
/// signed literal may reach the type's minimum), a float literal that is infinite in its type.
pub fn check_literals(prog: &Program, tcx: &crate::tcx::Tcx, body: &crate::typeck::Body) {
    use crate::ast::{ExprKind, LitKind, UnOp};
    use crate::types::TyKind;
    if !enabled() { return; }
    let pf = &prog.files[body.file as usize];
    let ast = &pf.ast;
    let n = body.expr_ty.len();
    let mut negated = std::collections::HashSet::new();
    for i in 0..n {
        let e = body.expr_at(i);
        if !body.expr_typed.get(i) { continue; }
        if let ExprKind::Unary(UnOp::Neg, x) = ast.expr(e).kind { negated.insert(x.0); }
    }
    for i in 0..n {
        let e = body.expr_at(i);
        if !body.expr_typed.get(i) { continue; }
        let ex = ast.expr(e);
        let kind = match ex.kind { ExprKind::Lit(k @ (LitKind::Int | LitKind::Float)) => k, _ => continue };
        let t = body.expr_ty[i];
        let text = std::str::from_utf8(pf.bytes(ex.lo, ex.hi)).unwrap_or("");
        let neg = negated.contains(&e.0);
        match (kind, tcx.tys.kind(t)) {
            (LitKind::Int, TyKind::Int(p)) => {
                let (bytes, signed) = crate::types::Types::int_info(*p);
                let Some(v) = crate::const_fold::parse_int_lit(text) else { continue };
                let bits = bytes * 8;
                let (max, min_abs) = if signed { ((1u128 << (bits - 1)) - 1, 1u128 << (bits - 1)) } else { (if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 }, 0) };
                let out = if neg && signed { v > min_abs } else { v > max };
                // a negated unsigned literal is E0600 (typeck), not this lint
                if out && !(neg && !signed) {
                    let ty = crate::typeck::ty_to_string(false, prog, tcx, t);
                    let lit = if neg { format!("-{}", text) } else { text.to_string() };
                    let range = if signed { format!("-{}..={}", min_abs, max) } else { format!("0..={}", max) };
                    emit(prog, body.def, &OVERFLOWING_LITERALS, body.file, ex.lo,
                        &format!("literal out of range for `{}` -- the literal `{}` does not fit into the type `{}` whose range is `{}`", ty, lit, ty, range));
                }
            }
            (LitKind::Float, TyKind::Float(_)) => {
                let ty = crate::typeck::ty_to_string(false, prog, tcx, t);
                let digits: String = text.chars().filter(|c| *c != '_').collect();
                let digits = digits.trim_end_matches("f32").trim_end_matches("f64");
                let inf = match ty.as_str() {
                    "f32" => digits.parse::<f32>().is_ok_and(|v| v.is_infinite()),
                    "f64" => digits.parse::<f64>().is_ok_and(|v| v.is_infinite()),
                    _ => false,
                };
                if inf {
                    emit(prog, body.def, &OVERFLOWING_LITERALS, body.file, ex.lo,
                        &format!("literal out of range for `{}` -- the literal `{}` does not fit into the type `{}` and will be converted to `{}::INFINITY`", ty, text, ty, ty));
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- shared helpers

/// Whether the code at `pos` is expanded from a macro of another crate, a built-in macro or a
/// derive: rustc does not report lints there (`Span::in_external_macro`).
pub fn external(prog: &Program, file: u32, pos: u32) -> bool {
    matches!(prog.files[file as usize].expansion_kind(pos), crate::parser::ExpnKind::ExternalRules | crate::parser::ExpnKind::Builtin)
}

/// The attribute list's leading path (`allow`, `repr`, `doc`, `rustfmt::skip`) as bytes.
pub fn attr_path(prog: &Program, file: u32, a: &Attr) -> Vec<u8> {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    let mut name: Vec<u8> = Vec::new();
    let mut i = lo;
    while i < hi && !matches!(crate::cfg::tok_str(p, i), b"(" | b"=" | b"[" | b"{") {
        name.extend_from_slice(crate::cfg::tok_str(p, i));
        i += 1;
    }
    name
}

/// An attribute's tokens joined by spaces (`crate_type = "lib"`).
pub fn attr_text(prog: &Program, file: u32, a: &Attr) -> String {
    let p = prog.src(file);
    (a.toks.lo as usize..a.toks.hi as usize).map(|i| String::from_utf8_lossy(crate::cfg::tok_str(p, i)).into_owned()).collect::<Vec<_>>().join(" ")
}

/// Whether one of `attrs` is the attribute `name` (a plain path).
pub fn has_attr(prog: &Program, file: u32, attrs: &[Attr], name: &str) -> bool {
    attrs.iter().any(|a| attr_path(prog, file, a) == name.as_bytes())
}

/// The words inside an attribute's parentheses (`repr(C, packed)`: `C`, `packed`), flat.
pub fn attr_args(prog: &Program, file: u32, a: &Attr) -> Vec<String> {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    let mut out = Vec::new();
    let mut i = lo;
    while i < hi && crate::cfg::tok_str(p, i) != b"(" { i += 1; }
    i += 1;
    while i < hi {
        let t = crate::cfg::tok_str(p, i);
        if !matches!(t, b"," | b")" | b"(") { out.push(String::from_utf8_lossy(t).into_owned()); }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- lint definitions
// (rustc_lint_defs builtin.rs, rustc_lint: name, default level, groups; `warnings` applies to all)

pub const NON_CAMEL_CASE_TYPES: Lint = lint("non_camel_case_types");
pub const NON_SNAKE_CASE: Lint = lint("non_snake_case");
pub const NON_UPPER_CASE_GLOBALS: Lint = lint("non_upper_case_globals");
pub const UNREACHABLE_CODE: Lint = lint("unreachable_code");
pub const UNUSED_VARIABLES: Lint = lint("unused_variables");
pub const UNUSED_ASSIGNMENTS: Lint = lint("unused_assignments");

/// The attributes of the statements, arms and blocks around the item `item` declared in the body
/// of `owner` (outermost first), as rustc's lint level builder sees them.
fn stmt_attrs_around(prog: &Program, owner: DefId, file: u32, item: crate::ast::ItemId) -> Vec<Attr> {
    use crate::ast::*;
    use crate::ast_visit::{self, Visitor};
    struct Find<'a> { ast: &'a Ast, item: ItemId, stack: Vec<Attr>, found: Option<Vec<Attr>> }
    impl<'a> Visitor<'a> for Find<'a> {
        fn ast(&self) -> &'a Ast { self.ast }
        fn enter_attrs(&mut self, attrs: List<Attr>) { self.stack.extend_from_slice(self.ast.list(attrs)); }
        fn leave_attrs(&mut self, attrs: List<Attr>) {
            let n = self.ast.list(attrs).len();
            self.stack.truncate(self.stack.len() - n);
        }
        fn stmt(&mut self, s: &Stmt, _b: BlockId) {
            if self.found.is_some() { return; }
            if let Stmt::Item(i) = *s {
                if i == self.item { self.found = Some(self.stack.clone()); }
                return;
            }
            ast_visit::walk_stmt(self, s);
        }
    }
    let od = prog.def(owner);
    let ast = &prog.files[od.file as usize].ast;
    if od.file != file { return Vec::new(); }
    let mut f = Find { ast, item, stack: Vec::new(), found: None };
    match ast.item(od.item).kind {
        ItemKind::Fn(..) => {
            if let Ok(Some(b)) = prog.fn_body(od.file, od.item) { f.block(b); }
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            if let Ok(Some(e)) = prog.const_body(od.file, od.item) { f.expr(e); }
        }
        _ => {}
    }
    f.found.unwrap_or_default()
}

/// The attributes of the statements, arms and blocks around the expression the macro call written at
/// `call` expanded to, in the body of `owner` (outermost first): rustc's lint level builder sees them
/// at the call's expansion.
pub fn attrs_at_call(prog: &Program, owner: DefId, file: u32, call: u32) -> Vec<Attr> {
    use crate::ast::*;
    use crate::ast_visit::{self, Visitor};
    struct Find<'a> { ast: &'a Ast, pf: &'a crate::parser::ParsedFile, call: u32, stack: Vec<Attr>, found: Option<Vec<Attr>> }
    impl<'a> Visitor<'a> for Find<'a> {
        fn ast(&self) -> &'a Ast { self.ast }
        fn enter_attrs(&mut self, attrs: List<Attr>) { self.stack.extend_from_slice(self.ast.list(attrs)); }
        fn leave_attrs(&mut self, attrs: List<Attr>) {
            let n = self.ast.list(attrs).len();
            self.stack.truncate(self.stack.len() - n);
        }
        fn expr(&mut self, e: ExprId) {
            if self.found.is_some() { return; }
            let lo = self.ast.expr(e).lo;
            if lo as usize >= self.pf.src_len as usize && self.pf.diagnostic_pos(lo) == self.call { self.found = Some(self.stack.clone()); return; }
            ast_visit::walk_expr(self, e);
        }
    }
    let od = prog.def(owner);
    if od.file != file || od.item.0 == u32::MAX { return Vec::new(); }
    let pf = &prog.files[od.file as usize];
    let ast = &pf.ast;
    let mut f = Find { ast, pf, call: pf.diagnostic_pos(call), stack: Vec::new(), found: None };
    match ast.item(od.item).kind {
        ItemKind::Fn(..) => {
            if let Ok(Some(b)) = prog.fn_body(od.file, od.item) { f.block(b); }
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            if let Ok(Some(e)) = prog.const_body(od.file, od.item) { f.expr(e); }
        }
        _ => {}
    }
    f.found.unwrap_or_default()
}

/// Whether impl `d` is the output of `#[derive]` (`#[automatically_derived]`; Relax expands
/// derives as built-in macro output).
pub fn is_automatically_derived(prog: &Program, d: DefId) -> bool {
    let def = prog.def(d);
    if def.kind != DefKind::Impl { return false; }
    let ast = &prog.files[def.file as usize].ast;
    let item = ast.item(def.item);
    has_attr(prog, def.file, ast.list(item.attrs), "automatically_derived")
        || prog.files[def.file as usize].expansion_kind(item.lo) == crate::parser::ExpnKind::Builtin
}

/// The attributes of the parameters, statements and arms around the binding written at `pos` in
/// the body of `owner` (outermost first).
pub fn attrs_at_binding(prog: &Program, owner: DefId, pos: u32) -> Vec<Attr> {
    use crate::ast::*;
    use crate::ast_visit::{self, Visitor};
    struct Find<'a> { ast: &'a Ast, pos: u32, stack: Vec<Attr>, found: Option<Vec<Attr>> }
    impl<'a> Visitor<'a> for Find<'a> {
        fn ast(&self) -> &'a Ast { self.ast }
        fn enter_attrs(&mut self, attrs: List<Attr>) { self.stack.extend_from_slice(self.ast.list(attrs)); }
        fn leave_attrs(&mut self, attrs: List<Attr>) {
            let n = self.ast.list(attrs).len();
            self.stack.truncate(self.stack.len() - n);
        }
        fn pat(&mut self, p: PatId) {
            if self.found.is_some() { return; }
            if let Pat::Ident { name, .. } = *self.ast.pat(p) {
                if name.lo == self.pos { self.found = Some(self.stack.clone()); return; }
            }
            ast_visit::walk_pat(self, p);
        }
    }
    let od = prog.def(owner);
    let ast = &prog.files[od.file as usize].ast;
    let mut f = Find { ast, pos, stack: Vec::new(), found: None };
    match ast.item(od.item).kind {
        ItemKind::Fn(sig, _, _) => {
            for p in ast.list(ast.fn_sig(sig).params) {
                f.enter_attrs(p.attrs);
                f.pat(p.pat);
                f.leave_attrs(p.attrs);
            }
            if f.found.is_none() { if let Ok(Some(b)) = prog.fn_body(od.file, od.item) { f.block(b); } }
        }
        ItemKind::Const(..) | ItemKind::Static(..) => {
            if let Ok(Some(e)) = prog.const_body(od.file, od.item) { f.expr(e); }
        }
        _ => {}
    }
    f.found.unwrap_or_default()
}

/// The innermost item of `file` whose source contains `pos` (the scope a lint reported from a
/// context without a definition belongs to); the crate root module when none does.
pub fn def_at(prog: &Program, file: u32, pos: u32) -> DefId {
    let ast = &prog.files[file as usize].ast;
    let mut best: Option<(u32, DefId)> = None;
    for i in 0..prog.defs.len() {
        let d = &prog.defs[i];
        if d.file != file || matches!(d.kind, DefKind::Mod | DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Prim(_) | DefKind::Field | DefKind::Variant | DefKind::MacroRules) { continue; }
        let it = ast.item(d.item);
        if pos >= it.lo && pos < it.hi {
            let size = it.hi - it.lo;
            if best.map_or(true, |(s, _)| size < s) { best = Some((size, DefId(i as u32))); }
        }
    }
    match best {
        Some((_, d)) => d,
        None => {
            let c = prog.file_crate[file as usize];
            prog.mods[prog.crates[c as usize].root_mod as usize].def
        }
    }
}

pub const AMBIGUOUS_GLOB_IMPORTS: Lint = lint("ambiguous_glob_imports");

/// The message of a privacy / ambiguity finding as an error, or None when it is the lint
/// `ambiguous_glob_imports` (emitted here under the levels in force at `pos`).
pub fn privacy_diag(prog: &Program, file: u32, pos: u32, e: &crate::program::PrivacyError) -> Option<String> {
    if !e.lint { return Some(prog.privacy_message(e)); }
    if enabled() {
        let def = def_at(prog, file, pos);
        let n = prog.syms.str(e.name);
        emit(prog, def, &AMBIGUOUS_GLOB_IMPORTS, file, pos, &format!("`{n}` is ambiguous"));
    }
    None
}

pub const AMBIGUOUS_IMPORT_VISIBILITIES: Lint = lint("ambiguous_import_visibilities");

/// What rustc's import resolution reports beyond Relax's own import errors (the replay in
/// front's program/rsim.rs): errors, and the lints `ambiguous_glob_imports` /
/// `ambiguous_import_visibilities` under the levels in force at the import.
pub fn import_resolution_diags(prog: &mut Program, own: &[bool]) -> Vec<String> {
    use relax_front::program::SimKind;
    let mut errors = Vec::new();
    if prog.sim_diags.is_none() { prog.own_crates = own.to_vec(); prog.run_import_sim(own); }
    let diags = prog.sim_diags.take().unwrap_or_default();
    let prog: &Program = prog;
    for d in diags {
        match d.kind {
            SimKind::Error if d.msg.starts_with("E0") => errors.push(prog.record_error(d.file, d.pos, "resolve", d.msg)),
            SimKind::Error => errors.push(prog.record_uncoded_error(d.file, d.pos, "resolve", d.msg)),
            SimKind::Lint(name) => {
                if !enabled() { continue; }
                let def = def_at(prog, d.file, d.pos);
                let l = match name { "ambiguous_import_visibilities" => AMBIGUOUS_IMPORT_VISIBILITIES, _ => AMBIGUOUS_GLOB_IMPORTS };
                emit(prog, def, &l, d.file, d.pos, &d.msg);
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_lints_resolve_to_their_new_name() {
        assert_eq!(current_name("bare_trait_object"), "bare_trait_objects");
        assert_eq!(current_name("bad_style"), "nonstandard_style");
        assert_eq!(current_name("dead_code"), "dead_code");
    }

    #[test]
    fn the_lint_table_knows_groups() {
        let l = crate::lint_table::LINTS.iter().find(|l| l.0 == "unused_imports").unwrap();
        assert!(l.3.contains(&"unused"));
        let f = crate::lint_table::LINTS.iter().find(|l| l.0 == "ambiguous_glob_imports").unwrap();
        assert!(f.3.contains(&"future_incompatible"));
        assert!(crate::lint_table::GROUPS.contains(&"nonstandard_style"));
    }
}

/// Whether `d` (or an item around it) was written by a derive, attribute or built-in macro:
/// rustc reports no lints inside their output (the spans are `in_external_macro`). A
/// `macro_rules!` expansion is the user's code and is judged.
pub fn generated(prog: &Program, d: DefId) -> bool {
    let mut cur = d;
    let mut guard = 0;
    while cur != NO_DEF && guard < 64 {
        guard += 1;
        let def = prog.def(cur);
        if matches!(def.kind, DefKind::Prim(_)) { return false; }
        if !matches!(def.kind, DefKind::Mod | DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Variant | DefKind::Field | DefKind::MacroRules) {
            let pf = &prog.files[def.file as usize];
            let it = pf.ast.item(def.item);
            if it.lo >= pf.src_len && matches!(pf.expansion_region_kind(it.lo), crate::parser::ExpnKind::Builtin | crate::parser::ExpnKind::ExternalRules) { return true; }
        }
        cur = def.parent;
    }
    false
}

/// `#[rustc_diagnostic_item = "name"]` of a definition.
pub fn diagnostic_item(prog: &Program, d: DefId) -> Option<String> {
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union | DefKind::Trait | DefKind::Fn | DefKind::AssocFn | DefKind::TypeAlias) { return None; }
    let ast = &prog.files[def.file as usize].ast;
    let p = prog.src(def.file);
    for a in ast.list(ast.item(def.item).attrs) {
        if attr_path(prog, def.file, a) != b"rustc_diagnostic_item" { continue; }
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        let mut i = lo;
        while i < hi && crate::cfg::tok_str(p, i) != b"=" { i += 1; }
        if i + 1 < hi {
            let t = crate::cfg::tok_str(p, i + 1);
            return Some(String::from_utf8_lossy(t).trim_matches('"').to_string());
        }
    }
    None
}

/// Whether `d` is the standard library item with diagnostic name `name` (`Box`, `PhantomData`).
pub fn is_diagnostic_item(prog: &Program, d: DefId, name: &str) -> bool {
    prog.name(d) == name && diagnostic_item(prog, d).as_deref() == Some(name)
}

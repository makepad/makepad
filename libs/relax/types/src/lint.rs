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

pub const UNUSED_MUT: Lint = Lint { name: "unused_mut", default: Level::Warn, default_2024: Level::Warn, groups: &["unused"] };
pub const OVERFLOWING_LITERALS: Lint = Lint { name: "overflowing_literals", default: Level::Deny, default_2024: Level::Deny, groups: &[] };
pub const UNUSED_UNSAFE: Lint = Lint { name: "unused_unsafe", default: Level::Warn, default_2024: Level::Warn, groups: &["unused"] };
pub const UNSAFE_OP_IN_UNSAFE_FN: Lint = Lint { name: "unsafe_op_in_unsafe_fn", default: Level::Allow, default_2024: Level::Warn, groups: &["rust_2024_compatibility"] };
pub const STATIC_MUT_REFS: Lint = Lint { name: "static_mut_refs", default: Level::Warn, default_2024: Level::Deny, groups: &["rust_2024_compatibility"] };
pub const UNREACHABLE_PATTERNS: Lint = Lint { name: "unreachable_patterns", default: Level::Warn, default_2024: Level::Warn, groups: &["unused"] };
pub const IRREFUTABLE_LET_PATTERNS: Lint = Lint { name: "irrefutable_let_patterns", default: Level::Warn, default_2024: Level::Warn, groups: &[] };

const LEVEL_ATTRS: &[(&[u8], Level)] = &[
    (b"allow", Level::Allow), (b"expect", Level::Allow), (b"warn", Level::Warn),
    (b"deny", Level::Deny), (b"forbid", Level::Forbid),
];

/// The lint names a level attribute (`deny(a, b)`) sets, with its level; tool lints
/// (`clippy::x`, `rustdoc::x`) are left out.
fn level_attr(prog: &Program, file: u32, a: &Attr) -> Option<(Level, Vec<String>)> {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    if hi <= lo + 2 { return None; }
    let head = crate::cfg::tok_str(p, lo);
    let level = LEVEL_ATTRS.iter().find(|(n, _)| *n == head)?.1;
    if crate::cfg::tok_str(p, lo + 1) != b"(" { return None; }
    let mut names = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut i = lo + 2;
    let mut done = |path: &mut Vec<String>| {
        if path.len() == 1 { names.push(std::mem::take(&mut path[0])); }
        path.clear();
    };
    while i < hi {
        let t = crate::cfg::tok_str(p, i);
        match t {
            b")" | b"," => { done(&mut path); if t == b")" { break; } }
            b"::" | b":" => {}
            _ => path.push(String::from_utf8_lossy(t).into_owned()),
        }
        i += 1;
    }
    Some((level, names))
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
        }
        d = dd.parent;
    }
    out
}

/// The level of `lint` at `def`.
pub fn level(prog: &Program, def: DefId, lint: &Lint) -> Level {
    level_in(prog, def, &[], lint)
}

/// The level of `lint` inside `def` under `inner`, the attributes of a node of its body (a
/// match arm) in the definition's file.
fn level_in(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint) -> Level {
    let mut scopes = scopes(prog, def);
    if !inner.is_empty() && def != NO_DEF {
        scopes.insert(0, (prog.def(def).file, inner.to_vec()));
    }
    // outermost first; within a list, later attributes override earlier ones
    let mut level = if prog.files[prog.def(def).file as usize].edition >= 2024 { lint.default_2024 } else { lint.default };
    let mut warnings: Option<Level> = None;
    let mut forbidden = false;
    let mut forbid_warnings = false;
    for (file, attrs) in scopes.iter().rev() {
        for a in attrs {
            let Some((l, names)) = level_attr(prog, *file, a) else { continue };
            for n in &names {
                if n == lint.name || lint.groups.contains(&n.as_str()) {
                    if !forbidden { level = l; forbidden = l == Level::Forbid; }
                } else if n == "warnings" && !forbid_warnings {
                    warnings = Some(l);
                    forbid_warnings = l == Level::Forbid;
                }
            }
        }
    }
    match (level, warnings) {
        (Level::Warn, Some(w)) => w,
        (l, _) => l,
    }
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

/// `emit` under the attributes `inner` of a node inside `def`'s body.
pub fn emit_in(prog: &Program, def: DefId, inner: &[Attr], lint: &Lint, file: u32, pos: u32, msg: &str) {
    if !enabled() { return; }
    let level = level_in(prog, def, inner, lint);
    if level == Level::Allow { return; }
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
    for (l, s) in all {
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

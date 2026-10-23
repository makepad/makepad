//! The checks rustc makes of proc-macro declarations (`makepad check` only):
//! - rustc_interface passes.rs: a `proc-macro` (or `bin`) crate type mixed with others;
//! - rustc_builtin_macros proc_macro_harness.rs `CollectProcMacros`: a proc-macro crate exports
//!   only its macro functions, which are `pub` and in the crate root; no `#[macro_export]`; one
//!   proc-macro attribute per function; the attributes need the `proc-macro` crate type;
//! - rustc_attr_parsing proc_macro_attrs.rs `parse_derive_like`: the `#[proc_macro_derive]` input
//!   (E0539 / E0565) and the `ambiguous_derive_helpers` lint;
//! - rustc_resolve macros.rs `check_reserved_macro_name`: no attribute macro named `cfg` / `cfg_attr`;
//! - rustc_passes check_attr.rs `check_proc_macro`: the function's signature is
//!   `fn(TokenStream) -> TokenStream` (two inputs for an attribute macro), safe, Rust ABI.

use crate::ast::{Attr, ItemKind, Vis};
use crate::lint::{self, Lint};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{Abi, Safety, TyKind};

pub const AMBIGUOUS_DERIVE_HELPERS: Lint = lint::lint("ambiguous_derive_helpers");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bang,
    Attr,
    Derive,
}

fn kind_of(path: &[u8]) -> Option<Kind> {
    match path {
        b"proc_macro" => Some(Kind::Bang),
        b"proc_macro_attribute" => Some(Kind::Attr),
        b"proc_macro_derive" => Some(Kind::Derive),
        _ => None,
    }
}

/// `#![crate_type = ".."]` of the crate's root file, deduplicated (rustc collect_crate_types).
fn crate_types(prog: &Program, krate: u32) -> Vec<String> {
    let rm = prog.crates[krate as usize].root_mod;
    let file = prog.mods[rm as usize].file;
    let mut out: Vec<String> = Vec::new();
    for a in prog.files[file as usize].ast.root_attrs.iter() {
        if lint::attr_path(prog, file, a) != b"crate_type" { continue; }
        if let Some(t) = lint::attr_text(prog, file, a).split('"').nth(1) {
            let t = if t == "lib" { "rlib" } else { t };
            if !out.iter().any(|x| x == t) { out.push(t.to_string()); }
        }
    }
    out
}

pub fn check(prog: &Program, tcx: &Tcx, own: &[bool]) -> Vec<String> {
    let mut errors = Vec::new();
    for k in 0..prog.crates.len() as u32 {
        if !own.get(k as usize).copied().unwrap_or(false) { continue; }
        check_crate(prog, tcx, k, &mut errors);
    }
    errors
}

fn check_crate(prog: &Program, tcx: &Tcx, k: u32, errors: &mut Vec<String>) {
    let c = &prog.crates[k as usize];
    let root = &prog.mods[c.root_mod as usize];
    let root_file = root.file;
    let types = crate_types(prog, k);
    let is_pm = c.proc_macro || types.iter().any(|t| t == "proc-macro");
    // (the session's crate types: the attributes, else a bin)
    if types.len() > 1 {
        if types.iter().any(|t| t == "bin") {
            errors.push(prog.record_error(root_file, 0, "resolve", "cannot mix `bin` crate type with others".to_string()));
        }
        if types.iter().any(|t| t == "proc-macro") {
            errors.push(prog.record_error(root_file, 0, "resolve", "cannot mix `proc-macro` crate type with others".to_string()));
        }
    }
    // a test harness build collects nothing (proc_macro_harness.rs `is_test_crate`)
    if c.cfg.set.iter().any(|s| s == "test") { return; }
    if is_pm {
        // (the program's macro table holds the crate's `macro_rules!`, expanded ones included)
        for defs in prog.macros.defs.values() {
            for d in defs.iter().filter(|d| d.exported && d.builtin == 0 && !d.def_file.is_empty()) {
                let Some(file) = prog.file_paths.iter().position(|p| *p == d.def_file) else { continue };
                if prog.file_crate.get(file).copied() != Some(k) || d.def_pos >= prog.files[file].src_len { continue; }
                errors.push(prog.record_error(file as u32, d.def_pos, "resolve", "cannot export macro_rules! macros from a `proc-macro` crate type currently".to_string()));
            }
        }
    }
    let token_stream = token_stream_ty(prog, tcx);
    let root_def = root.def;
    let mut stubs: Vec<(DefId, String)> = Vec::new();
    // every item of the crate (rustc's visitor walks the expanded crate: nested modules and the
    // items of fn bodies; associated items are not items here)
    for i in 0..prog.defs.len() {
        let did = DefId(i as u32);
        let def = prog.def(did);
        if def.krate != k || prog.item_defs.get(&(def.file, def.item.0)) != Some(&did) { continue; }
        if matches!(def.kind, DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy | DefKind::Variant) { continue; }
        let f = &prog.files[def.file as usize];
        if def.file != root_file && f.src_len == 0 { continue; }
        let item = f.ast.item(def.item);
        let attrs: Vec<Attr> = f.ast.list(item.attrs).to_vec();
        let in_root = def.parent == root_def;
        if let ItemKind::MacroRules(_) = item.kind { continue; }
        let mut found: Option<(&Attr, Vec<u8>)> = None;
        let mut dup = false;
        for a in &attrs {
            let p = lint::attr_path(prog, def.file, a);
            if kind_of(&p).is_none() { continue; }
            if let Some((_, prev)) = &found {
                let msg = if p == *prev {
                    format!("only one `#[{}]` attribute is allowed on any given function", String::from_utf8_lossy(&p))
                } else {
                    format!("`#[{}]` and `#[{}]` attributes cannot both be applied\n                            to the same function", String::from_utf8_lossy(&p), String::from_utf8_lossy(prev))
                };
                errors.push(prog.record_error(def.file, f.tok(a.toks.lo).lo, "resolve", msg));
                dup = true;
                break;
            }
            found = Some((a, p));
        }
        if dup { continue; }
        let Some((attr, path)) = found else {
            if is_pm && in_root && matches!(item.vis, Vis::Pub) && !matches!(item.kind, ItemKind::Impl(..) | ItemKind::Mac(..) | ItemKind::ForeignMod(..)) {
                errors.push(prog.record_error(def.file, item.lo, "resolve", "`proc-macro` crate types currently cannot export any items other than functions tagged with `#[proc_macro]`, `#[proc_macro_derive]`, or `#[proc_macro_attribute]`".to_string()));
            }
            continue;
        };
        let kind = kind_of(&path).unwrap();
        // (a non-function's attribute is the attribute target check's)
        if def.kind != DefKind::Fn { continue; }
        let name = String::from_utf8_lossy(&path).into_owned();
        let attr_pos = f.tok(attr.toks.lo).lo;
        if !is_pm {
            errors.push(prog.record_error(def.file, attr_pos, "resolve", format!("the `#[{}]` attribute is only usable with crates of the `proc-macro` crate type", name)));
            continue;
        }
        // build_reduced_graph proc_macro_stub: the macro (the derive's trait name, else the fn's
        // name) is defined in the macro namespace of the fn's module (E0428 for a second one)
        let (macro_name, macro_pos) = if kind == Kind::Derive {
            let Some(t) = derive_input(prog, def.file, did, attr, errors) else { continue };
            (f.tok_text(t).to_string(), f.tok(t).lo)
        } else {
            (f.text(item.name).to_string(), item.name.lo)
        };
        if stubs.iter().any(|(p, n)| *p == def.parent && *n == macro_name) {
            errors.push(prog.record_error(def.file, macro_pos, "resolve", format!("E0428: the name `{}` is defined multiple times", macro_name)));
        } else {
            stubs.push((def.parent, macro_name));
        }
        if !(in_root && matches!(item.vis, Vis::Pub)) {
            let msg = if !in_root {
                format!("functions tagged with `#[{}]` must currently reside in the root of the crate", name)
            } else {
                format!("functions tagged with `#[{}]` must be `pub`", name)
            };
            errors.push(prog.record_error(def.file, item.lo, "resolve", msg));
        }
        // build_reduced_graph proc_macro_stub + check_reserved_macro_name
        if kind == Kind::Attr && matches!(f.text(item.name), "cfg" | "cfg_attr") {
            errors.push(prog.record_error(def.file, item.name.lo, "resolve", format!("name `{}` is reserved in attribute namespace", f.text(item.name))));
        }
        if let Some(ts) = token_stream {
            if !signature_ok(prog, tcx, did, ts, kind) {
                let what = match kind { Kind::Bang => "function-like", Kind::Attr => "attribute", Kind::Derive => "derive" };
                errors.push(prog.record_error(def.file, item.lo, "typeck", format!("{} proc macro has incorrect signature", what)));
            }
        }
    }
}

/// `proc_macro::TokenStream` when the program has the `proc_macro` crate (rustc: the
/// `TokenStream` diagnostic item).
fn token_stream_ty(prog: &Program, tcx: &Tcx) -> Option<crate::types::TyId> {
    let pm = prog.crates.iter().position(|c| prog.syms.str(c.name) == "proc_macro")?;
    let sym = prog.syms.get("TokenStream")?;
    let d = prog.lookup_in_module(prog.crates[pm].root_mod, sym, true, &mut Vec::new())?;
    (prog.def(d).kind == DefKind::Struct).then(|| tcx.tys.intern(TyKind::Adt(d, tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY)))
}

/// check_attr.rs check_proc_macro: the signature at fresh arguments, normalized, equals
/// `fn(TokenStream{, TokenStream}) -> TokenStream` (safe, Rust ABI, not variadic). A signature
/// that is not well formed is not judged.
fn signature_ok(prog: &Program, tcx: &Tcx, d: DefId, ts: crate::types::TyId, kind: Kind) -> bool {
    use crate::next_solver::fulfill::ObligationCtxt;
    let Some(sig) = tcx.fn_sig(prog, d) else { return true };
    if sig.params.iter().chain(std::iter::once(&sig.ret)).any(|&t| tcx.tys.has_error(t)) { return true; }
    let header = &sig.header;
    let inputs = if kind == Kind::Attr { 2 } else { 1 };
    if header.safety != Safety::Safe || header.abi != Abi::Rust || header.c_variadic || sig.params.len() != inputs { return false; }
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, crate::tcx::TypingMode::non_body_analysis(), d);
    let env = tcx.intern_param_env(crate::next_solver::ParamEnv { clauses: Vec::new().into() });
    let fresh = infcx.fresh_args_for_item(prog, d);
    let mut ocx = ObligationCtxt::new(&infcx, prog);
    for &t in sig.params.iter().chain(std::iter::once(&sig.ret)) {
        let t = crate::next_solver::dyn_compatibility::liberate_late_bound_regions(tcx, d, t);
        let t = tcx.instantiate_ty(prog, d, &fresh, t);
        let Ok(t) = ocx.deeply_normalize_ty(&env, t) else { return true };
        if infcx.at(prog, env.clone()).eq(ts, t).is_err() { return false; }
    }
    true
}

/// rustc_attr_parsing parse_derive_like for `#[proc_macro_derive(Trait, attributes(a, b))]`:
/// false (and the error) when the input is malformed.
fn derive_input(prog: &Program, file: u32, did: DefId, a: &Attr, errors: &mut Vec<String>) -> Option<u32> {
    let f = &prog.files[file as usize];
    let at = f.tok(a.toks.lo).lo;
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    let tok = |i: usize| -> &[u8] { crate::cfg::tok_str(prog.src(file), i) };
    let malformed = |code: &str| format!("{}: malformed `proc_macro_derive` attribute input", code);
    // the path, then a parenthesized list
    let mut i = lo;
    while i < hi && !matches!(tok(i), b"(" | b"=" | b"[" | b"{") { i += 1; }
    if i >= hi || tok(i) != b"(" {
        errors.push(prog.record_error(file, at, "resolve", malformed("E0539")));
        return None;
    }
    // the list's items: (first token, end) split at top-level commas
    let close = hi - 1;
    let mut items: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut start) = (0i32, i + 1);
    for j in i + 1..close {
        match tok(j) {
            b"(" | b"[" | b"{" => depth += 1,
            b")" | b"]" | b"}" => depth -= 1,
            b"," if depth == 0 => { items.push((start, j)); start = j + 1; }
            _ => {}
        }
    }
    if start < close { items.push((start, close)); }
    let is_lit = |t: &[u8]| t.first().is_some_and(|c| c.is_ascii_digit() || *c == b'"' || *c == b'\'') || t == b"true" || t == b"false" || t.starts_with(b"b\"") || t.starts_with(b"r\"") || t.starts_with(b"r#");
    let word = |t: &[u8]| t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_' || *c >= 0x80);
    // a name that can be raw (Symbol::can_be_raw): not `_` or a path keyword
    let raw_ok = |t: &[u8]| !matches!(t, b"_" | b"self" | b"Self" | b"super" | b"crate");
    // one meta item at [s, e): Err(code) or Ok((name token, args start))
    let meta = |s: usize, e: usize| -> Result<(usize, usize), &'static str> {
        if is_lit(tok(s)) { return Err("E0565"); }
        // a path: words joined by `::`
        let mut j = s;
        if !word(tok(j)) && tok(j) != b"::" { return Err("E0539"); }
        let mut segs = 0;
        while j < e && (word(tok(j)) || tok(j) == b"::") { if tok(j) != b"::" { segs += 1; } j += 1; }
        if segs != 1 || tok(s) == b"::" { return Err("E0539"); }
        Ok((s, j))
    };
    let Some(&(s0, e0)) = items.first() else {
        errors.push(prog.record_error(file, at, "resolve", malformed("E0539")));
        return None;
    };
    let trait_tok = match meta(s0, e0) {
        Err(code) => { errors.push(prog.record_error(file, at, "resolve", malformed(code))); return None; }
        Ok((n, args)) => {
            if !raw_ok(tok(n)) { errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None; }
            if args < e0 { errors.push(prog.record_error(file, at, "resolve", malformed("E0565"))); return None; }
            n as u32
        }
    };
    if let Some(&(s1, e1)) = items.get(1) {
        match meta(s1, e1) {
            Err(code) => { errors.push(prog.record_error(file, at, "resolve", malformed(code))); return None; }
            Ok((n, args)) => {
                if tok(n) != b"attributes" { errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None; }
                // expect_list
                if args >= e1 || tok(args) != b"(" || tok(e1 - 1) != b")" { errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None; }
                let (mut depth, mut st) = (0i32, args + 1);
                let mut helpers: Vec<(usize, usize)> = Vec::new();
                for j in args + 1..e1 - 1 {
                    match tok(j) {
                        b"(" | b"[" | b"{" => depth += 1,
                        b")" | b"]" | b"}" => depth -= 1,
                        b"," if depth == 0 => { helpers.push((st, j)); st = j + 1; }
                        _ => {}
                    }
                }
                if st < e1 - 1 { helpers.push((st, e1 - 1)); }
                for (hs, he) in helpers {
                    // a literal here is "expected a valid identifier" (E0539)
                    if is_lit(tok(hs)) { errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None; }
                    let Ok((n, hargs)) = meta(hs, he) else {
                        // a path's no-args check comes first: `m::a` has none, so the identifier check reports it
                        errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None;
                    };
                    if hargs < he { errors.push(prog.record_error(file, at, "resolve", malformed("E0565"))); return None; }
                    if !raw_ok(tok(n)) { errors.push(prog.record_error(file, at, "resolve", malformed("E0539"))); return None; }
                    let name = String::from_utf8_lossy(tok(n)).into_owned();
                    // rustc_feature::is_builtin_attr_name: BUILTIN_ATTRIBUTES (not the attribute macros)
                    const MACRO_ATTRS: &[&str] = &["alloc_error_handler", "autodiff_forward", "autodiff_reverse", "bench", "cfg_accessible", "cfg_eval", "define_opaque", "derive", "derive_const", "eii", "eii_declaration", "eii_shared_macro", "global_allocator", "test", "test_case", "unsafe_eii"];
                    if crate::builtin_attrs::is_builtin_attr(&name) && !MACRO_ATTRS.contains(&name.as_str()) {
                        lint::emit(prog, did, &AMBIGUOUS_DERIVE_HELPERS, file, f.tok(n as u32).lo, "there exists a built-in attribute with the same name");
                    }
                }
            }
        }
    }
    if items.len() > 2 {
        errors.push(prog.record_error(file, at, "resolve", malformed("E0565")));
        return None;
    }
    Some(trait_tok)
}

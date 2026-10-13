//! `makepad std-surface <std dir> <out dir> <commit>`: the public type surface of Relax's std
//! (user decision S1, 2026-10-05). The public front end (the Rust frontend) type-checks
//! against it; the JIT keeps the full std. The surface is machine-generated ENTIRELY: every
//! output file is this generator's output, never edited. Whatever it cannot express is fixed
//! here, not in the output.
//!
//! What it keeps: every item, signature, impl, trait, type, const and static of core, alloc,
//! std, proc_macro and the substituted crates (smallvec, arrayvec), and `substitutes.txt` /
//! `CREDITS.md`. What it removes: fn bodies, replaced by `{ loop {} }` (a diverging body
//! type-checks for any return type), except where a caller's types depend on the body:
//! - `const fn` (array lengths and const values are evaluated from them),
//! - fns returning `impl Trait` (the hidden type comes from the body),
//! - `async fn` (the future type comes from the body).
//! The surface is for the front end, which runs no code (no constant evaluation by execution, no
//! proc-macro host): it cannot run under the JIT -- a constant or macro that executes a removed
//! body would loop. The JIT keeps the full std.
//! fn bodies inside `macro_rules!` transcribers go too (the transcriber's tokens, so a body that
//! splices metavariables is removed whole).
//!
//! `makepad std-surface --check <std dir> <out dir> <commit>`: regenerates in memory and fails
//! (exit 1) when any exported file differs or is missing / extra (the staleness gate).

use crate::ast::{Ast, ItemId, ItemKind, Ty};

/// Directories of Relax's std that are the library (the rest are test harnesses and benches).
const LIB_DIRS: [&str; 6] = ["core", "alloc", "std", "proc_macro", "smallvec", "arrayvec"];
/// Plain files copied as they are.
const COPIED: [&str; 2] = ["substitutes.txt", "CREDITS.md"];

#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub lines_in: usize,
    pub lines_out: usize,
    pub stripped: usize,
    pub kept_const: usize,
    pub kept_impl_trait: usize,
    pub kept_async: usize,
}

fn header(commit: &str) -> String {
    format!("// generated from Relax std {} by `makepad std-surface` -- do not edit: regenerate (fn bodies removed; the public type surface)\n", commit)
}

fn returns_impl_trait(ast: &Ast, t: Option<crate::ast::TyId>) -> bool {
    fn walk(ast: &Ast, t: crate::ast::TyId, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        match ast.ty(t) {
            Ty::ImplTrait(_) => true,
            Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Paren(x) => walk(ast, *x, depth + 1),
            Ty::Array(x, _) => walk(ast, *x, depth + 1),
            Ty::Tuple(v) => ast.list(*v).iter().any(|x| walk(ast, *x, depth + 1)),
            Ty::Path(p) => ast.segs(p.segs).iter().any(|s| match ast.gargs(s.args()) {
                Some(crate::ast::GenericArgs::Angle(a)) => ast.list(*a).iter().any(|g| matches!(g, crate::ast::GenericArg::Type(x) if walk(ast, *x, depth + 1))),
                _ => false,
            }),
            _ => false,
        }
    }
    t.map_or(false, |t| walk(ast, t, 0))
}

/// One fn item with a body.
struct FnBody {
    /// why the body stays: const fn (0), `impl Trait` return (1), async fn (2); None: removed
    own: Option<u8>,
    block: (u32, u32),
}

/// Fn bodies inside `macro_rules!` transcribers (`(..) => { .. fn f(..) -> T { body } .. }`):
/// the body brace groups of non-const fns, as byte ranges. Matchers are left alone (a literal
/// `fn` there is part of the macro's syntax).
fn macro_fn_bodies(toks: &[crate::lexer::Tok], range: crate::ast::TokRange, out: &mut Vec<(u32, u32)>) {
    use crate::lexer::T;
    let (lo, hi) = (range.lo as usize, range.hi as usize);
    let is_open = |k: T| matches!(k, T::OpenParen | T::OpenBracket | T::OpenBrace);
    let is_close = |k: T| matches!(k, T::CloseParen | T::CloseBracket | T::CloseBrace);
    // the end (exclusive) of the delimited group opening at i
    let group_end = |i: usize| -> usize {
        let mut d = 0i32;
        let mut j = i;
        while j < hi {
            if is_open(toks[j].kind) {
                d += 1;
            } else if is_close(toks[j].kind) {
                d -= 1;
                if d == 0 {
                    return j + 1;
                }
            }
            j += 1;
        }
        hi
    };
    let mut i = lo;
    while i < hi {
        // rule: matcher group, `=>`, transcriber group, optional `;`
        if !is_open(toks[i].kind) {
            i += 1;
            continue;
        }
        let m_end = group_end(i);
        if m_end >= hi || toks[m_end].kind != T::FatArrow || m_end + 1 >= hi || !is_open(toks[m_end + 1].kind) {
            i = m_end;
            continue;
        }
        let t_lo = m_end + 1;
        let t_end = group_end(t_lo);
        let mut j = t_lo + 1;
        while j + 1 < t_end {
            if toks[j].kind == T::KwFn && !(j > t_lo && toks[j - 1].kind == T::KwConst) {
                // the parameter list, then the first `{` at this level (no body before `;`)
                let mut k = j + 1;
                while k < t_end && toks[k].kind != T::OpenParen {
                    k += 1;
                }
                if k < t_end {
                    k = group_end(k);
                }
                while k < t_end && !matches!(toks[k].kind, T::OpenBrace | T::Semi) {
                    if is_open(toks[k].kind) {
                        k = group_end(k);
                    } else {
                        k += 1;
                    }
                }
                if k < t_end && toks[k].kind == T::OpenBrace {
                    let e = group_end(k);
                    out.push((toks[k].lo, toks[e - 1].hi));
                    j = e;
                    continue;
                }
            }
            j += 1;
        }
        i = t_end;
    }
}

/// The fn bodies of one file.
fn scan(ast: &Ast, toks: &[crate::lexer::Tok], items: &[ItemId], fns: &mut Vec<FnBody>) {
    for &id in items {
        let it = ast.item(id);
        match &it.kind {
            ItemKind::Fn(sig, Some(b), _) => {
                let sig = ast.fn_sig(*sig);
                let own = if sig.konst {
                    Some(0)
                } else if sig.asyncness {
                    Some(2)
                } else if returns_impl_trait(ast, sig.ret) {
                    Some(1)
                } else {
                    None
                };
                let blk = &ast.blocks[b.0 as usize];
                fns.push(FnBody { own, block: (blk.lo, blk.hi) });
            }
            ItemKind::MacroRules(r) => {
                let mut v = Vec::new();
                macro_fn_bodies(toks, *r, &mut v);
                fns.extend(v.into_iter().map(|block| FnBody { own: None, block }));
            }
            ItemKind::Mod(Some(v)) => scan(ast, toks, ast.list(*v), fns),
            ItemKind::ForeignMod(r) => scan(ast, toks, ast.list(ast.row(*r).items), fns),
            ItemKind::Impl(r) => scan(ast, toks, ast.list(ast.row(*r).items), fns),
            ItemKind::Trait(r) => scan(ast, toks, ast.list(ast.row(*r).items), fns),
            _ => {}
        }
    }
}

/// The surface text of one source file, given the bodies to remove.
fn emit(src: &[u8], commit: &str, mut ranges: Vec<(u32, u32)>) -> Result<Vec<u8>, String> {
    ranges.sort();
    let mut out = header(commit).into_bytes();
    let mut at = 0usize;
    for (lo, hi) in ranges {
        let (lo, hi) = (lo as usize, hi as usize);
        if lo < at || hi > src.len() || src.get(lo) != Some(&b'{') || src.get(hi.wrapping_sub(1)) != Some(&b'}') {
            return Err(format!("internal: fn body range {}..{} is not a brace block -- fix the generator", lo, hi));
        }
        out.extend_from_slice(&src[at..lo]);
        out.extend_from_slice(b"{ loop {} }");
        at = hi;
    }
    out.extend_from_slice(&src[at..]);
    Ok(out)
}

fn collect(dir: &std::path::Path, rel: &str, out: &mut Vec<String>) {
    let Ok(rd) = crate::vfs::read_dir(dir.join(rel)) else { return };
    for (name, is_dir) in rd {
        let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if is_dir {
            collect(dir, &r, out);
        } else if name.ends_with(".rs") {
            out.push(r);
        }
    }
}

/// (relative path, contents) of the whole surface.
///
/// A body stays when the type checker can need it: its fn is a const fn (constant values,
/// array lengths), an async fn or returns `impl Trait` (the type comes from the body). The
/// front end runs no code, so no other body is ever read.
pub fn generate(sess: &std::sync::Arc<crate::Session>, std_dir: &str, commit: &str) -> Result<(Vec<(String, Vec<u8>)>, Stats), Vec<String>> {
    let base = std::path::Path::new(std_dir);
    let mut files = Vec::new();
    for d in LIB_DIRS {
        collect(base, d, &mut files);
    }
    let mut st = Stats { files: 0, lines_in: 0, lines_out: 0, stripped: 0, kept_const: 0, kept_impl_trait: 0, kept_async: 0 };
    let mut errs = Vec::new();
    let mut out = Vec::new();
    for f in &files {
        let src = match crate::vfs::read(base.join(f)) {
            Ok(s) => s,
            Err(e) => {
                errs.push(format!("{}: {}", f, e));
                continue;
            }
        };
        match surface_text(sess, &src, commit, &mut st) {
            Ok(s) => {
                st.files += 1;
                st.lines_in += src.iter().filter(|&&c| c == b'\n').count();
                st.lines_out += s.iter().filter(|&&c| c == b'\n').count();
                out.push((f.clone(), s));
            }
            Err(e) => errs.push(format!("{}: {}", f, e)),
        }
    }
    for c in COPIED {
        if let Ok(b) = crate::vfs::read(base.join(c)) {
            out.push((c.to_string(), b));
        }
    }
    if errs.is_empty() {
        Ok((out, st))
    } else {
        Err(errs)
    }
}

/// One source file's surface.
pub fn surface_text(sess: &std::sync::Arc<crate::Session>, src: &[u8], commit: &str, st: &mut Stats) -> Result<Vec<u8>, String> {
    let p = crate::parser::parse_source_mode(sess, src, 2021, false).map_err(|(pos, msg)| {
        let (l, c) = crate::lexer::line_col(src, pos);
        format!("{}:{}: {}", l, c, msg)
    })?;
    let mut fns = Vec::new();
    scan(&p.ast, p.toks.as_slice(), &p.ast.root_items, &mut fns);
    let mut ranges = Vec::new();
    for b in fns {
        match b.own {
            Some(0) => st.kept_const += 1,
            Some(1) => st.kept_impl_trait += 1,
            Some(_) => st.kept_async += 1,
            None => {
                ranges.push(b.block);
                st.stripped += 1;
            }
        }
    }
    // a body nested in a removed body goes with it
    ranges.sort();
    let mut outer: Vec<(u32, u32)> = Vec::new();
    for r in ranges {
        if outer.last().map_or(true, |o| r.0 >= o.1) {
            outer.push(r);
        }
    }
    emit(src, commit, outer)
}

pub fn cmd_std_surface(sess: &std::sync::Arc<crate::Session>, args: &[String]) -> i32 {
    let check = args.first().map(String::as_str) == Some("--check");
    let a = if check { &args[1..] } else { args };
    if a.len() != 3 {
        eprintln!("usage: makepad std-surface [--check] <makepad run std dir> <out dir> <std commit>");
        return 2;
    }
    let (files, st) = match generate(sess, &a[0], &a[2]) {
        Ok(x) => x,
        Err(e) => {
            for l in e {
                eprintln!("std-surface: cannot express: {}", l);
            }
            return 1;
        }
    };
    let out = std::path::Path::new(&a[1]);
    if check {
        let mut bad = Vec::new();
        let mut want: Vec<String> = Vec::new();
        for (rel, data) in &files {
            want.push(rel.clone());
            match std::fs::read(out.join(rel)) {
                Ok(b) if &b == data => {}
                Ok(_) => bad.push(format!("stale {}", rel)),
                Err(_) => bad.push(format!("missing {}", rel)),
            }
        }
        let mut have = Vec::new();
        for d in LIB_DIRS {
            collect(out, d, &mut have);
        }
        for h in have {
            if !want.contains(&h) {
                bad.push(format!("extra {}", h));
            }
        }
        for b in &bad {
            println!("std-surface: {}", b);
        }
        println!("std-surface check: {} files, {} differ", files.len(), bad.len());
        return if bad.is_empty() { 0 } else { 1 };
    }
    for (rel, data) in &files {
        let p = out.join(rel);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&p, data) {
            eprintln!("std-surface: {}: {}", p.display(), e);
            return 1;
        }
    }
    println!(
        "std-surface: {} files, {} -> {} lines; fn bodies removed {}, kept: const fn {}, impl-Trait return {}, async {}",
        st.files, st.lines_in, st.lines_out, st.stripped, st.kept_const, st.kept_impl_trait, st.kept_async
    );
    0
}

#[cfg(test)]
mod tests {
    fn surf(src: &str) -> String {
        let mut st = super::Stats::default();
        let out = super::surface_text(&std::sync::Arc::new(crate::Session::default()), src.as_bytes(), "c0", &mut st).unwrap();
        let s = String::from_utf8(out).unwrap();
        s.split_once('\n').unwrap().1.to_string()
    }

    #[test]
    fn bodies() {
        assert_eq!(surf("pub fn f(x: u32) -> u32 { x + 1 }"), "pub fn f(x: u32) -> u32 { loop {} }");
        assert_eq!(surf("pub const fn f() -> usize { 4 }"), "pub const fn f() -> usize { 4 }");
        assert_eq!(surf("pub fn it(v: &[u8]) -> impl Iterator<Item = &u8> { v.iter() }"), "pub fn it(v: &[u8]) -> impl Iterator<Item = &u8> { v.iter() }");
        assert_eq!(surf("pub async fn a() -> u8 { 1 }"), "pub async fn a() -> u8 { 1 }");
        assert_eq!(surf("impl S { fn m(&self) { g() } } trait T { fn d(&self) -> u8 { 0 } fn r(&self); }"), "impl S { fn m(&self) { loop {} } } trait T { fn d(&self) -> u8 { loop {} } fn r(&self); }");
        assert_eq!(surf("fn outer() { fn inner() { 1; } inner() }"), "fn outer() { loop {} }");
        assert_eq!(surf("const C: usize = 3; static S: u8 = 1; struct A { x: [u8; C] }"), "const C: usize = 3; static S: u8 = 1; struct A { x: [u8; C] }");
    }

    #[test]
    fn macro_transcribers() {
        let src = "macro_rules! m { ($t:ident fn { $($b:tt)* }) => { impl $t { fn f(&self) -> u8 { $($b)* } const fn k() -> u8 { 1 } } }; }";
        let want = "macro_rules! m { ($t:ident fn { $($b:tt)* }) => { impl $t { fn f(&self) -> u8 { loop {} } const fn k() -> u8 { 1 } } }; }";
        assert_eq!(surf(src), want);
    }

    #[test]
    fn header() {
        let mut st = super::Stats::default();
        let out = String::from_utf8(super::surface_text(&std::sync::Arc::new(crate::Session::default()), b"fn f() {}", "abc123", &mut st).unwrap()).unwrap();
        assert!(out.starts_with("// generated from Relax std abc123 by `makepad std-surface` -- do not edit"));
    }
}

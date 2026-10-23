//! `eval`, `static` and `locals`: looking at a running (paused, parked) program's values from a
//! module's scope.
//!
//! - `eval [in=<crate::path>] [panic=P<n>] [mutate=1] <expr>`: the expression is compiled as the
//!   body of a fn in a new child module of the named module (modules.rs load_in_module: its
//!   private items, `use`s and `crate::` paths are in scope), built from tokens (the agent's
//!   expression lexed, inside `::std::relax::probe::__eval(&(<expr>))`: a token-level expansion,
//!   no source text is written), and run once on the inspect thread in the app's domain (a parked
//!   panic's domain, else the app's). The answer is the value's `{:#?}` and its type; a type that
//!   is not Debug answers its size, address and bytes. Without the default scope of a parked panic
//!   (its innermost program frame's module) `in=` names the module. Reads only: `unsafe`, `&mut`
//!   and assignments outside a `let` are refused unless mutate=1 (as for probes, a fn the
//!   expression calls runs as it is).
//! - `static <path>`: a static's address in the app's domain, type, size and bytes; one that
//!   holds a reference or a raw pointer has its target followed: the allocation it points into
//!   (a static, a data object, a writable block, code, a malloc block) and whether what it
//!   describes fits in it. `debug=1` adds the Debug form (an `eval` of the static).
//! - `locals [panic=P<n>|break=K<n>|thread=T<n>] [frame=N]`: a stop's frames, and per frame and
//!   inline level its fn, line and named locals with their values (vars.rs).

use super::{json_str, short, Out, Req};
use crate::jit::{FnKey, Unit};
use crate::pm_tokens::{Delimiter, Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};
use crate::program::{DefId, DefKind};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

thread_local! {
    /// (type, value) the eval fn reported (tags 0 and 1)
    static RESULT: std::cell::RefCell<(Option<String>, Option<String>)> = const { std::cell::RefCell::new((None, None)) };
    static IN_EVAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Is this thread running an `eval` (its panics are the eval's answer, never parked)?
pub fn in_eval() -> bool {
    IN_EVAL.try_with(|c| c.get()).unwrap_or(false)
}

/// std::relax::probe::__eval's answer: tag 0 the type, 1 the value.
pub extern "C" fn relax_probe_result(tag: u32, t: *const u8, n: usize) {
    let s = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(t, n) }).into_owned();
    let _ = RESULT.try_with(|r| {
        let mut r = r.borrow_mut();
        if tag == 0 {
            r.0 = Some(s);
        } else {
            r.1 = Some(s);
        }
    });
}

// ------------------------------------------------------------------ where: scope and domain

/// The module whose scope an eval uses and the domain base it runs in, with a note on why.
struct Place {
    module: u32,
    module_path: String,
    domain: Option<u32>,
    base: u64,
}

/// The innermost program frame (not core / alloc / std) of `frames`: (slot, fn def).
fn program_frame(u: &Unit, frames: &[u64]) -> Option<(usize, u32, DefId)> {
    for (k, &pc) in frames.iter().enumerate() {
        let Some(id) = u.find_fn(pc) else { continue };
        let mut key = &u.fns[id as usize].key;
        // a closure is in its parent's scope
        for _ in 0..8 {
            match key {
                FnKey::Closure(_, _, p) | FnKey::Plain(p) | FnKey::Sandbox(p) if (*p as usize) < u.fns.len() => key = &u.fns[*p as usize].key,
                _ => break,
            }
        }
        if let FnKey::Inst(d, _) = key {
            let k_name = u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name);
            if !matches!(k_name, "core" | "alloc" | "std") {
                return Some((k, id, *d));
            }
        }
    }
    None
}

fn domain_base(u: &Unit, domain: Option<u32>) -> (Option<u32>, u64) {
    let d = domain.or_else(crate::run::app_domain);
    match d.and_then(crate::domain::info) {
        Some(i) if i.base != 0 => (d, i.base),
        _ => (None, u.syms.dom0()),
    }
}

/// The scope (`in=` or the parked panic's innermost program frame) and the domain (the panic's,
/// else the app's). On the owner.
fn place(u: &Unit, req: &Req) -> Result<Place, String> {
    let panic = req.get("panic").and_then(|p| p.trim_start_matches('P').parse::<u64>().ok());
    let parked = super::events::parked_frames(panic);
    if panic.is_some() && parked.is_none() {
        return Err(format!("no parked panic P{}", panic.unwrap()));
    }
    let module = match req.get("in") {
        Some(p) => crate::modules::module_named(u, p).ok_or_else(|| format!("no module `{}` (name it from its crate: `my_crate::a::b`)", p))?,
        None => {
            let Some((_, _, frames, _)) = &parked else { return Err("no parked panic: name the module with in=<crate::path>".into()) };
            let (_, _, d) = program_frame(u, frames).ok_or("the parked panic has no program frame: name the module with in=<crate::path>")?;
            crate::modules::scope_of(u, d)
        }
    };
    let (domain, base) = domain_base(u, parked.as_ref().and_then(|p| p.1));
    let module_path = u.prog.def_path(u.prog.mods[module as usize].def);
    Ok(Place { module, module_path, domain, base })
}

// ------------------------------------------------------------------ eval

fn ident(s: &str) -> TokenTree {
    TokenTree::Ident(Ident::new(s, Span::DUMMY))
}

fn punct(ch: char, spacing: Spacing) -> TokenTree {
    TokenTree::Punct(Punct { ch, spacing, span: Span::DUMMY })
}

fn group(delim: Delimiter, trees: Vec<TokenTree>) -> TokenTree {
    TokenTree::Group(Group { delim, stream: TokenStream { trees }, span: Span::DUMMY })
}

/// `::a::b::c` as tokens.
fn path_tokens(segs: &[&str]) -> Vec<TokenTree> {
    let mut v = Vec::new();
    for s in segs {
        v.push(punct(':', Spacing::Joint));
        v.push(punct(':', Spacing::Alone));
        v.push(ident(s));
    }
    v
}

/// `pub fn __relax_eval() { ::std::relax::probe::<helper>(&(<expr>)); }`
fn wrapper(expr: TokenStream, helper: &str) -> TokenStream {
    let mut body = path_tokens(&["std", "relax", "probe", helper]);
    body.push(group(Delimiter::Parenthesis, vec![punct('&', Spacing::Alone), TokenTree::Group(Group { delim: Delimiter::Parenthesis, stream: expr, span: Span::DUMMY })]));
    body.push(punct(';', Spacing::Alone));
    TokenStream { trees: vec![ident("pub"), ident("fn"), ident("__relax_eval"), group(Delimiter::Parenthesis, Vec::new()), group(Delimiter::Brace, body)] }
}

/// The agent's expression as tokens whose spans point into `src` (file 0 of the eval's file).
fn lex_expr(src: &str) -> Result<TokenStream, String> {
    let mut toks = Vec::new();
    crate::lexer::lex(src.as_bytes(), 2021, &mut toks).map_err(|_| "the expression does not lex".to_string())?;
    let n = toks.len().saturating_sub(1);
    crate::builtins::to_stream(src.as_bytes(), &toks, 0, n, &|lo, hi| Span { lo, hi, ..Span::DUMMY }).ok_or_else(|| "unbalanced delimiters in the expression".to_string())
}

/// What in `trees` writes: `unsafe`, `&mut`, an assignment outside a `let` or a compound
/// assignment (a token-level check; None: reads only).
fn writes(trees: &[TokenTree]) -> Option<&'static str> {
    let mut in_let = false;
    for (i, t) in trees.iter().enumerate() {
        match t {
            TokenTree::Group(g) => {
                if let Some(w) = writes(&g.stream.trees) {
                    return Some(w);
                }
            }
            TokenTree::Ident(id) if id.name == "unsafe" => return Some("an `unsafe` block"),
            TokenTree::Ident(id) if id.name == "let" => in_let = true,
            TokenTree::Ident(id) if id.name == "mut" && i > 0 && matches!(&trees[i - 1], TokenTree::Punct(p) if p.ch == '&') => return Some("`&mut`"),
            TokenTree::Punct(p) if p.ch == ';' => in_let = false,
            TokenTree::Punct(p) if p.ch == '=' => {
                // `==`, `=>`: this '=' joins the next
                if p.spacing == Spacing::Joint {
                    continue;
                }
                let prev = if i > 0 { match &trees[i - 1] { TokenTree::Punct(q) if q.spacing == Spacing::Joint => Some(q.ch), _ => None } } else { None };
                match prev {
                    // `==` (second), `!=`, `<=`, `>=`
                    Some('=') | Some('!') => {}
                    Some('<') | Some('>') => {
                        // `<<=` / `>>=` are compound assignments
                        if i > 1 && matches!(&trees[i - 2], TokenTree::Punct(q) if q.spacing == Spacing::Joint && (q.ch == '<' || q.ch == '>')) {
                            return Some("a compound assignment");
                        }
                    }
                    Some('+' | '-' | '*' | '/' | '%' | '^' | '&' | '|') => return Some("a compound assignment"),
                    _ if in_let => {}
                    _ => return Some("an assignment"),
                }
            }
            _ => {}
        }
    }
    None
}

/// Compiles `expr` in `module` as an eval fn (on the owner): its callable entry. A type that is
/// not Debug compiles again with the raw view.
fn compile(up: *mut Unit, module: u32, src: &str, expr: &TokenStream) -> Result<(u64, bool), String> {
    let mut last = String::new();
    for (helper, raw) in [("__eval", false), ("__eval_raw", true)] {
        let id = NEXT.fetch_add(1, SeqCst);
        let name = format!("__relax_eval_{}", id);
        let path = super::staged_path(&format!("eval_{}.rs", id));
        let _ = std::fs::write(&path, src);
        let u = unsafe { &mut *up };
        let v = u.prog.child_parse_snapshot(module, &name, &path).parse_tokens(src.as_bytes().to_vec(), wrapper(expr.clone(), helper))?;
        match unsafe { crate::modules::load_in_module(up, module, &name, v) } {
            Ok((child, _)) => {
                let u = unsafe { &mut *up };
                let sym = u.prog.syms.get("__relax_eval").ok_or("no eval fn")?;
                let d = *u.prog.mods[child as usize].values.get(&sym).ok_or("no eval fn")?;
                let slot = u.fn_id(FnKey::Inst(d, Vec::new()));
                return Ok((u.entry(slot), raw));
            }
            Err(e) => {
                last = e.join("\n");
                // only a missing Debug impl goes on to the raw view
                if raw || !last.contains("Debug") {
                    return Err(last);
                }
            }
        }
    }
    Err(last)
}

/// Runs an eval fn on this thread in the domain at `base`: (type, value).
fn run(up: *mut Unit, entry: u64, base: u64) -> Result<(String, String), String> {
    run_arg(up, entry, base, 0)
}

/// `run` of an eval fn taking one argument word (`locals`: `__eval::<T>(&copy)`).
fn run_arg(up: *mut Unit, entry: u64, base: u64, arg: u64) -> Result<(String, String), String> {
    let _ = RESULT.try_with(|r| *r.borrow_mut() = (None, None));
    let _ = IN_EVAL.try_with(|c| c.set(true));
    let r = unsafe { crate::jit::call_jit_in(up, base, entry, arg) };
    let _ = IN_EVAL.try_with(|c| c.set(false));
    let (t, v) = RESULT.try_with(|r| std::mem::take(&mut *r.borrow_mut())).unwrap_or_default();
    match r {
        Ok(_) => Ok((t.unwrap_or_default(), v.unwrap_or_default())),
        Err(p) => Err(format!("the expression panicked: {} ({})", p.message, p.kind)),
    }
}

/// eval's work for `src` (the expression's text) in the request's place: (type, value, place).
fn eval_text(req: &Req, src: &str, mutate: bool) -> Result<(String, String, bool, Place), String> {
    eval_tokens(req, src, lex_expr(src)?, mutate)
}

/// eval of `expr` (tokens; `src`: the eval file's text, what their spans point into).
fn eval_tokens(req: &Req, src: &str, expr: TokenStream, mutate: bool) -> Result<(String, String, bool, Place), String> {
    if expr.trees.is_empty() {
        return Err("usage: eval [in=<crate::path>] [panic=P<n>] [mutate=1] <expr>".into());
    }
    if !mutate {
        if let Some(w) = writes(&expr.trees) {
            return Err(format!("the expression may write ({}): add mutate=1 to allow it", w));
        }
    }
    let up = crate::jit::current_unit();
    if up.is_null() {
        return Err("no running unit".into());
    }
    let req2 = Req { cmd: req.cmd.clone(), pos: Vec::new(), kv: req.kv.clone() };
    let text = src.to_string();
    let r = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || -> Result<(u64, bool, Place), String> {
        let u = unsafe { &*up };
        let p = place(u, &req2)?;
        let (entry, raw) = compile(up, p.module, &text, &expr)?;
        Ok((entry, raw, p))
    });
    let (entry, raw, p) = r.unwrap_or_else(|| Err(format!("the owner did not take the request in 30 s ({})", crate::owner::serving_note())))?;
    let (t, v) = run(up, entry, p.base)?;
    Ok((t, v, raw, p))
}

/// `eval [in=<crate::path>] [panic=P<n>] [mutate=1] <expr>`
pub fn cmd_eval(req: &Req, out: &mut Out) {
    // the expression keeps its spaces in `expr` (the client sends it so); a typed line is its words
    let src = req.get("expr").map(str::to_string).unwrap_or_else(|| req.pos.join(" "));
    match eval_text(req, &src, req.get("mutate") == Some("1")) {
        Ok((t, v, raw, p)) => {
            out.measured(&format!("eval in {}", p.module_path), "owner: compile; this thread: run", &[]);
            out.line(&format!(
                "{{\"in\":{},\"domain\":{},\"type\":{},{}:{}}}",
                json_str(&p.module_path),
                p.domain.map_or("null".to_string(), |d| d.to_string()),
                json_str(&t),
                if raw { "\"raw\"" } else { "\"value\"" },
                json_str(&short(&v, 8000))
            ));
        }
        Err(e) => out.err(&e),
    }
}

// ------------------------------------------------------------------ static

/// What memory at `a` is, from the program's tables (on the owner): (description, the
/// allocation's [start, end) when known).
fn describe_addr(u: &Unit, a: u64) -> (String, Option<(u64, u64)>) {
    if a == 0 {
        return ("null".into(), None);
    }
    if let Some((&start, &(d, size))) = u.static_spans.range(..=a).next_back() {
        if a < start + size.max(1) && u.statics.get(&d) == Some(&start) {
            return (format!("static {} +{} (its storage: {} bytes)", crate::lower::display_path(&u.prog, d), a - start, size), Some((start, start + size)));
        }
    }
    if let Some((start, len, _, mutable)) = u.syms.object_of(a) {
        return (format!("{} data object at {:#x} +{} ({} bytes)", if mutable { "writable" } else { "read-only" }, start, a - start, len), Some((start, start + len)));
    }
    if let Some(s) = u.syms.rw_sym_at(a) {
        return (format!("writable block (symbol {}) +{}", s.id, s.off), None);
    }
    if let Some(id) = u.find_fn(a) {
        return (format!("code of {}", u.fns[id as usize].name), None);
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_size(p: *const u8) -> usize;
        }
        let n = unsafe { malloc_size(a as *const u8) };
        if n > 0 {
            return (format!("malloc block of {} bytes", n), Some((a, a + n as u64)));
        }
    }
    let readable = super::threads::read_mem(a, 1).is_some();
    (format!("not a Relax static or data object (host memory or a heap interior; {})", if readable { "readable" } else { "NOT mapped" }), None)
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 3);
    for (i, x) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{:02x}", x));
    }
    s
}

/// The static a path names: `krate::a::NAME`, or `NAME` in the request's scope.
fn static_named(u: &Unit, req: &Req, path: &str) -> Result<DefId, String> {
    let (prefix, name) = match path.rsplit_once("::") {
        Some((p, n)) => (Some(p), n),
        None => (None, path),
    };
    let sym = u.prog.syms.get(name).ok_or_else(|| format!("no item `{}`", name))?;
    let module = match prefix {
        Some(p) => crate::modules::module_named(u, p).ok_or_else(|| format!("no module `{}` (name it from its crate: `my_crate::a::b`)", p))?,
        None => place(u, req)?.module,
    };
    let d = u.prog.lookup_in_scope(module, sym, false).ok_or_else(|| format!("no `{}` in {}", name, u.prog.def_path(u.prog.mods[module as usize].def)))?;
    match u.prog.def(d).kind {
        DefKind::Static => Ok(d),
        k => Err(format!("`{}` is a {:?}, not a static", path, k)),
    }
}

/// What `static` resolved on the owner: the static, its module's path, its type and layout.
struct StaticInfo {
    path: String,
    name: String,
    module: String,
    ty: String,
    size: u64,
    align: u64,
    /// the pointee of a reference / raw pointer type: (count words: slice or str, element size, shown)
    pointee: Option<(bool, u64, String)>,
}

/// `static <path> [panic=P<n>] [debug=1]`: the address comes from the program itself (an eval of
/// `addr_of!(NAME)` in the static's module and the app's domain), so it is the storage the
/// program's code reads, whatever placed it.
pub fn cmd_static(req: &Req, out: &mut Out) {
    let Some(path) = req.pos.first().cloned() else {
        out.err("usage: static <crate::path::NAME> [debug=1]");
        return;
    };
    let up = crate::jit::current_unit();
    if up.is_null() {
        out.err("no running unit");
        return;
    }
    let req2 = Req { cmd: req.cmd.clone(), pos: Vec::new(), kv: req.kv.clone() };
    let info = crate::owner::call_within(crate::owner::Class::Program, 10_000, move || -> Result<StaticInfo, String> {
        use crate::types::TyKind;
        let u = unsafe { &mut *up };
        let d = static_named(u, &req2, &path)?;
        let module = u.prog.mods[u.prog.def(d).scope as usize].normal;
        let t = u.tcx.tys.erase_regions(u.tcx.const_ty(&u.prog, d).ok_or("the static has no type")?);
        let show = |t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t);
        let l = u.layout_of(t);
        let pointee = match u.tcx.tys.kind(t) {
            TyKind::Ref(_, inner, _) | TyKind::Ptr(_, inner) => Some(match u.tcx.tys.kind(*inner) {
                TyKind::Slice(e) => (true, u.layout_of(*e).size as u64, format!("[{}]", show(*e))),
                TyKind::Str => (true, 1, "str".to_string()),
                _ => (false, u.layout_of(*inner).size as u64, show(*inner)),
            }),
            _ => None,
        };
        Ok(StaticInfo {
            path: crate::lower::display_path(&u.prog, d),
            name: u.prog.syms.str(u.prog.def(d).name).to_string(),
            module: u.prog.def_path(u.prog.mods[module as usize].def),
            ty: show(t),
            size: l.size as u64,
            align: l.align as u64,
            pointee,
        })
    });
    let info = match info.unwrap_or_else(|| Err(format!("the owner did not take the request in 10 s ({})", crate::owner::serving_note()))) {
        Ok(i) => i,
        Err(e) => return out.err(&e),
    };
    // its address as the program's code computes it
    let mut kv: Vec<(String, String)> = req.kv.iter().filter(|k| k.0 == "panic").cloned().collect();
    kv.push(("in".into(), info.module.clone()));
    let r2 = Req { cmd: "eval".into(), pos: Vec::new(), kv };
    let mut expr = path_tokens(&["core", "ptr", "addr_of"]);
    expr.push(punct('!', Spacing::Alone));
    expr.push(group(Delimiter::Parenthesis, vec![ident(&info.name)]));
    expr.push(ident("as"));
    expr.push(ident("usize"));
    let (addr, domain) = match eval_tokens(&r2, &info.name, TokenStream { trees: expr }, false) {
        Ok((_, v, _, p)) => (v.trim().parse::<u64>().unwrap_or(0), p.domain),
        Err(e) => return out.err(&format!("the static's address: {}", e)),
    };
    let bytes = super::threads::read_mem(addr, (info.size as usize).min(256)).unwrap_or_default();
    out.measured("static", "eval: its address; owner: program tables; memory reads", &[]);
    out.line(&format!(
        "{{\"static\":{},\"type\":{},\"size\":{},\"align\":{},\"address\":\"{:#x}\",\"domain\":{},\"bytes\":{}}}",
        json_str(&info.path),
        json_str(&info.ty),
        info.size,
        info.align,
        addr,
        domain.map_or("null".to_string(), |d| d.to_string()),
        json_str(&hex(&bytes))
    ));
    if let (Some((counted, elem, target)), true) = (&info.pointee, bytes.len() >= 8) {
        let w = |k: usize| bytes.get(k * 8..k * 8 + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        let ptr = w(0).unwrap_or(0);
        let count = if *counted { w(1) } else { None };
        let n = count.map_or(*elem, |c| c.saturating_mul(*elem));
        let (what, alloc) = crate::owner::call_within(crate::owner::Class::Job, 5_000, move || describe_addr(unsafe { &*up }, ptr)).unwrap_or_else(|| ("(owner busy)".into(), None));
        let fits = match alloc {
            Some((s, e)) if ptr >= s && ptr.saturating_add(n) <= e => "yes".to_string(),
            Some((s, e)) => format!("NO: it reads [{:#x}, {:#x}), the allocation is [{:#x}, {:#x}): {} bytes past its end", ptr, ptr.saturating_add(n), s, e, ptr.saturating_add(n).saturating_sub(e)),
            None => "unknown (the allocation's size is not known)".to_string(),
        };
        let head = super::threads::read_mem(ptr, (n as usize).min(64));
        out.line(&format!(
            "{{\"points_to\":\"{:#x}\",\"target\":{},{}\"target_bytes\":{},\"allocation\":{},\"fits\":{},\"first_bytes\":{},\"first_text\":{}}}",
            ptr,
            json_str(target),
            count.map_or(String::new(), |c| format!("\"len\":{},", c)),
            n,
            json_str(&what),
            json_str(&fits),
            head.as_ref().map_or("null".to_string(), |h| json_str(&hex(h))),
            head.as_ref().map_or("null".to_string(), |h| json_str(&h.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect::<String>()))
        ));
    }
    if req.get("debug") == Some("1") {
        let mut kv: Vec<(String, String)> = req.kv.iter().filter(|k| k.0 == "panic").cloned().collect();
        kv.push(("in".into(), info.module.clone()));
        let r3 = Req { cmd: "eval".into(), pos: Vec::new(), kv };
        match eval_tokens(&r3, &info.name, TokenStream { trees: vec![ident(&info.name)] }, false) {
            Ok((_, v, raw, _)) => out.line(&format!("{{{}:{}}}", if raw { "\"raw\"" } else { "\"debug\"" }, json_str(&short(&v, 8000)))),
            Err(e) => out.line(&format!("{{\"debug_error\":{}}}", json_str(&short(&e, 2000)))),
        }
    }
}

// ------------------------------------------------------------------ locals

/// `locals [panic=P<n>|break=K<n>|thread=T<n>|main] [frame=N]`: a stop's frames, then for every
/// frame (or frame N) each inline level (innermost first: the fn, where it is, its code) with
/// its named locals' VALUES as the code's variable table places them at that call (a register,
/// a frame slot, a constant; vars.rs), `{:#?}` by the type's Debug (its bytes otherwise), or why
/// a local has no value there ("optimised out"). Nothing is recompiled; no probe runs.
pub fn cmd_locals(req: &Req, out: &mut Out) {
    let up = crate::jit::current_unit();
    if up.is_null() {
        return out.err("no running unit");
    }
    // the stop: its label, register block or thread registers, domain
    enum Src {
        Block(Vec<u64>),
        Thread(u64, u64, Vec<(usize, u64)>),
        None(String),
    }
    let (label, src, domain): (String, Src, Option<u32>) = if let Some(k) = req.get("break") {
        let id = k.trim_start_matches('K').parse::<u64>().ok();
        match super::instrument::parked_break(id) {
            Some((id, _, regs)) if !regs.is_empty() => (format!("\"break\":\"K{}\"", id), Src::Block(regs), None),
            Some((id, _, _)) => (format!("\"break\":\"K{}\"", id), Src::None("this breakpoint hit keeps no register block (prebuilt code)".into()), None),
            None => return out.err("no parked breakpoint hit (`watch on=break`)"),
        }
    } else if let Some(sel) = req.get("thread") {
        let want = super::pick_threads(sel);
        let list = super::threads::list();
        let Some(t) = list.iter().find(|t| want(t)) else { return out.err(&format!("no thread {}", sel)) };
        if t.os_id == super::threads::self_os_id() {
            return out.err("that is the inspect thread");
        }
        if super::events::panic_parked(t.os_id) || super::instrument::parked_break_of(t.os_id).is_some() {
            return out.err("that thread is parked at a panic or breakpoint: `locals panic=P<n>` / `locals break=K<n>` read its stop (with its registers)");
        }
        if !super::instrument::stopped(t) {
            return out.err("that thread is running: `pause` first (its stack moves while it runs)");
        }
        match super::threads::regs_now(t) {
            Some((pc, fp, ints)) => (format!("\"thread\":\"{}\"", super::tid_label(t.os_id)), Src::Thread(pc, fp, ints), None),
            None => return out.err("this thread's registers are not readable here (pause it first: `pause`)"),
        }
    } else {
        let panic = req.get("panic").and_then(|p| p.trim_start_matches('P').parse::<u64>().ok());
        let Some((id, dom, _, _)) = super::events::parked_frames(panic) else {
            return out.err("no parked panic (locals reads a stop: a parked panic `panics`, break=K<n>, thread=T<n>)");
        };
        let regs = super::events::parked_regs(Some(id)).map(|x| x.1).unwrap_or_default();
        let src = if regs.is_empty() { Src::None("this panic keeps no register block (a fault, not a panic call)".into()) } else { Src::Block(regs) };
        (format!("\"panic\":\"P{}\"", id), src, dom)
    };
    let want: Option<usize> = req.get("frame").and_then(|f| f.parse().ok());
    // on the owner: frames, levels, the locals' bytes; Debug fns for their types
    let lab = label.clone();
    let r = crate::owner::call_within(crate::owner::Class::Program, 30_000, move || -> Result<(Vec<String>, Vec<(String, Vec<super::vars::Local>, Vec<Option<u64>>)>), String> {
        let u = unsafe { &*up };
        let stop = match src {
            Src::Block(b) => super::vars::Stop::from_block(u, &b).ok_or("the register block is short")?,
            Src::Thread(pc, fp, ints) => super::vars::Stop::from_thread(u, pc, fp, &ints),
            Src::None(why) => return Err(why),
        };
        if stop.frames.is_empty() {
            return Err("the stop has no JIT frames".into());
        }
        let mut rows = Vec::new();
        let mut res = super::codever::Resolver::new();
        for (k, &(pc, _)) in stop.frames.iter().enumerate() {
            let (name, at, code) = match super::codever::describe_pc(u, &mut res, pc) {
                Some(c) => {
                    let (n, at, _, code) = super::codever::frame_fields(u, &c);
                    (n, at, code)
                }
                None => ("-".to_string(), String::new(), "null".to_string()),
            };
            rows.push(format!("{{\"frame\":{},\"fn\":{},\"at\":{},\"code\":{}{}}}", k, json_str(&short(&name, 160)), json_str(&at), code, if want == Some(k) { ",\"chosen\":true" } else { "" }));
        }
        let mut levels = Vec::new();
        for (k, lv) in super::vars::levels(u, &stop, want) {
            for l in lv {
                let head = format!(
                    "{{{},\"frame\":{},\"inline_level\":{},\"fn\":{},\"at\":{}{}{}",
                    lab,
                    k,
                    l.level,
                    json_str(&short(&l.name, 160)),
                    json_str(&l.at),
                    l.inlined_into.as_ref().map_or(String::new(), |s| format!(",\"inlined_into\":{}", json_str(&short(s, 160)))),
                    l.note.as_ref().map_or(String::new(), |s| format!(",\"note\":{}", json_str(s))),
                );
                let entries: Vec<Option<u64>> = l.locals.iter().map(|x| match (&x.got, x.t) {
                    (super::vars::Got::Bytes(..), Some(t)) => super::vars::debug_entry(u, t),
                    _ => None,
                }).collect();
                levels.push((head, l.locals, entries));
            }
        }
        Ok((rows, levels))
    });
    let (rows, levels) = match r.unwrap_or_else(|| Err(format!("the owner did not take the request in 30 s ({})", crate::owner::serving_note()))) {
        Ok(x) => x,
        Err(e) => return out.err(&e),
    };
    // the values' Debug forms, run here (the inspect thread) in the program's domain on copies
    let (_, base) = domain_base(unsafe { &*up }, domain);
    out.measured(&format!("locals of {}", label.replace('"', "")), "owner: frames, variable tables, checked bodies; this thread: Debug of copies", &[]);
    for l in rows {
        out.line(&l);
    }
    for (head, locals, entries) in levels {
        let mut ls = Vec::with_capacity(locals.len());
        for (x, e) in locals.iter().zip(&entries) {
            let shown = match (&x.got, e) {
                (super::vars::Got::Bytes(b, _), Some(entry)) => {
                    // an aligned copy (the frame's bytes stay as they are)
                    let mut buf = vec![0u128; (b.len() + 15) / 16 + 1];
                    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), buf.as_mut_ptr() as *mut u8, b.len()) };
                    Some(run_arg(up, *entry, base, buf.as_ptr() as u64).map(|v| v.1))
                }
                _ => None,
            };
            ls.push(super::vars::local_json(x, shown.as_ref()));
        }
        out.line(&format!("{},\"locals\":[{}]}}", head, ls.join(",")));
    }
}

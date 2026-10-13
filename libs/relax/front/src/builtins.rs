//! Native built-in macros (builtins lane).
//!
//! - proc macros (derives and function-like) go through the sandboxed JIT host (pmhost.rs);
//! - derive input preparation (cfg_eval, derive attrs removed);
//! - the std built-ins those expansions use: file!, line!, column!, module_path!, env!,
//!   option_env!, stringify!, concat!, include_str!, include_bytes!.
//!
//! Expansion produces source text that the parser lexes and parses in place, like a
//! `macro_rules!` expansion (positions map back to the call site). A `macro_rules!` of the
//! same name, visible at the call, wins (it is looked up first).

use crate::cfg::CfgSet;
use crate::lexer::{Tok, T};
use crate::parser::lazy_bodies::{SegRef, TokSeq};
use crate::pm::{Delimiter, Group, LineColumn, Punct, Spacing, Span, TokenStream, TokenTree};
#[cfg(test)]
use crate::pm::{Ident, Literal};
use crate::source::{SourceSpan, TokenOrigin};

// ---------------------------------------------------------------- per-file context

/// What the built-ins need to know about the file being parsed.
pub struct FileCtx {
    /// crate name as Relax knows it (`makepad_widgets`)
    pub krate: String,
    /// path of the file (as Relax loaded it)
    pub path: String,
    /// `file!()`: relative to the workspace root when inside it (as cargo passes it to rustc)
    pub file_macro: String,
    /// module path of the file's root (`makepad_widgets::button`)
    pub module_path: String,
    pub cfg: CfgSet,
    /// `CARGO_MANIFEST_DIR`, `CARGO_PKG_NAME`, `CARGO_PKG_VERSION`
    pub manifest_dir: String,
    pub pkg_name: String,
    pub pkg_version: String,
    lines: std::cell::OnceCell<Vec<u32>>,
    continuations: std::cell::OnceCell<Vec<u32>>,
}

// Process-wide, by the file's directory: files parse on many worker threads, and the walk up
// to the manifest (a stat per level) and the manifest reads are the same for every file.
static BY_DIR: relax_session::sync::Mutex<Option<crate::fxhash::HashMap<String, (String, String, String, String)>>> = relax_session::sync::Mutex::new(None);

/// Per crate root: (manifest dir, package name, version, workspace root).
fn crate_meta(path: &str) -> (String, String, String, String) {
    let fdir = crate::program::dir_of(path);
    if let Some(v) = BY_DIR.lock().unwrap().as_ref().and_then(|m| m.get(fdir).cloned()) {
        return v;
    }
    let v = crate_meta_uncached(path);
    BY_DIR.lock().unwrap().get_or_insert_with(Default::default).insert(fdir.to_string(), v.clone());
    v
}

fn crate_meta_uncached(path: &str) -> (String, String, String, String) {
    crate::gen_local! {
        static CACHE: std::cell::RefCell<crate::fxhash::HashMap<String, (String, String, String, String)>>;
    }
    let mut dir = std::path::Path::new(path).parent().map(|p| p.to_path_buf());
    let mut manifest: Option<std::path::PathBuf> = None;
    while let Some(d) = dir {
        if crate::vfs::exists(d.join("Cargo.toml")) {
            manifest = Some(d.clone());
            break;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    let mdir = match manifest {
        Some(m) => m,
        None => return (crate::program::dir_of(path).to_string(), String::new(), String::new(), String::new()),
    };
    let key = mdir.to_string_lossy().into_owned();
    if let Some(v) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return v;
    }
    let toml = crate::vfs::read_to_string(mdir.join("Cargo.toml")).unwrap_or_default();
    let (name, mut version, ver_ws) = package_fields(&toml);
    // workspace root: the nearest ancestor (or self) whose Cargo.toml has [workspace]
    let mut ws = String::new();
    let mut d = Some(mdir.clone());
    while let Some(x) = d {
        if let Ok(t) = crate::vfs::read_to_string(x.join("Cargo.toml")) {
            if t.lines().any(|l| l.trim() == "[workspace]") {
                ws = x.to_string_lossy().into_owned();
                if ver_ws {
                    version = workspace_version(&t);
                }
                break;
            }
        }
        d = x.parent().map(|p| p.to_path_buf());
    }
    let v = (key.clone(), name, version, ws);
    CACHE.with(|c| c.borrow_mut().insert(key, v.clone()));
    v
}

/// `[package]` name and version (`version.workspace = true` -> the bool).
fn package_fields(toml: &str) -> (String, String, bool) {
    let (mut name, mut version, mut ws) = (String::new(), String::new(), false);
    let mut in_pkg = false;
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_pkg = l == "[package]";
            continue;
        }
        if !in_pkg {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let (k, v) = (k.trim(), v.trim());
            match k {
                "name" => name = v.trim_matches('"').to_string(),
                "version" => version = v.trim_matches('"').to_string(),
                "version.workspace" => ws = v == "true",
                _ => {}
            }
        }
    }
    (name, version, ws)
}

fn workspace_version(toml: &str) -> String {
    let mut in_pkg = false;
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_pkg = l == "[workspace.package]";
            continue;
        }
        if in_pkg {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == "version" {
                    return v.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    String::new()
}

/// `"..".parse::<TokenStream>()` on Relax's token model: Relax's lexer (call-site spans).
pub fn lex_stream(s: &str) -> Option<TokenStream> {
    let mut toks = Vec::new();
    crate::lexer::lex(s.as_bytes(), 2021, &mut toks).ok()?;
    let cs = Span::call_site();
    to_stream(s.as_bytes(), &toks, 0, toks.len().saturating_sub(1), &|_lo, _hi| cs)
}

/// The path `file!()` names for source file `path` (as cargo passes it to rustc: relative to
/// the workspace root for a workspace member, else as given); `Location::caller().file()` and
/// panic locations name the same path.
pub fn file_macro_path(path: &str) -> String {
    let ws = crate_meta(path).3;
    if !ws.is_empty() {
        if let Some(rel) = crate::vfs::relative_path(path, &ws) { return rel; }
    }
    path.to_string()
}

impl FileCtx {
    pub fn new(krate: &str, path: &str, module_path: &str, cfg: &CfgSet) -> FileCtx {
        crate::pm::set_lexer(lex_stream);
        let (manifest_dir, pkg_name, pkg_version, _ws) = crate_meta(path);
        let file_macro = file_macro_path(path);
        FileCtx {
            krate: krate.to_string(),
            path: path.to_string(),
            file_macro,
            module_path: module_path.to_string(),
            cfg: cfg.clone(),
            manifest_dir,
            pkg_name,
            pkg_version,
            lines: std::cell::OnceCell::new(),
            continuations: std::cell::OnceCell::new(),
        }
    }

    fn continuations(&self, src: &[u8]) -> &Vec<u32> {
        self.continuations.get_or_init(|| continuation_bytes(src))
    }

    fn line_starts(&self, src: &[u8]) -> &Vec<u32> {
        self.lines.get_or_init(|| line_starts(src))
    }

    /// crate::lexer::line_col(src, pos) from the cached line starts
    pub(crate) fn byte_line_col(&self, src: &[u8], pos: u32) -> (usize, usize) {
        crate::lexer::line_col_at(self.line_starts(src), src.len(), pos)
    }
}

fn line_starts(src: &[u8]) -> Vec<u32> {
    let mut v = vec![0u32];
    for (i, &c) in src.iter().enumerate() {
        if c == b'\n' {
            v.push(i as u32 + 1);
        }
    }
    v
}

/// 1-based line and char column of byte `pos` in `src` (line starts `ls`).
fn line_col(src: &[u8], ls: &[u32], pos: u32) -> LineColumn {
    let pos = pos.min(src.len() as u32);
    let line = match ls.binary_search(&pos) {
        Ok(i) => i,
        Err(i) => i - 1,
    };
    let mut col = 1;
    for &b in &src[ls[line] as usize..pos as usize] {
        if b & 0xC0 != 0x80 {
            col += 1;
        }
    }
    LineColumn { line: line + 1, column: col }
}

// ---------------------------------------------------------------- byte payloads

/// Marker prefix of an `include_bytes!` literal whose bytes live in the payload table.
pub const PAYLOAD_MARK: &str = "\\0relax-include:";

static PAYLOADS: relax_session::sync::Mutex<Option<crate::fxhash::HashMap<String, std::sync::Arc<Vec<u8>>>>> = relax_session::sync::Mutex::new(None);

pub(crate) struct ZoneTaken {
    payloads: Option<crate::fxhash::HashMap<String, std::sync::Arc<Vec<u8>>>>,
    dirs: Option<crate::fxhash::HashMap<String, (String, String, String, String)>>,
}

pub(crate) fn zone_take() -> ZoneTaken {
    let mut payloads = PAYLOADS.lock().unwrap_or_else(|e| e.into_inner());
    let mut dirs = BY_DIR.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken { payloads: std::mem::take(&mut *payloads), dirs: std::mem::take(&mut *dirs) };
    #[cfg(test)]
    { assert!(payloads.is_none()); assert!(dirs.is_none()); }
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut payloads = PAYLOADS.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *payloads, taken.payloads));
    let mut dirs = BY_DIR.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *dirs, taken.dirs));
}

/// The payload's key: its hash and length (the same text whichever thread or order includes it;
/// files parse in parallel), disambiguated in the unlikely case two contents collide.
fn register_payload(data: Vec<u8>) -> String {
    use std::hash::Hasher;
    let mut h = crate::fxhash::FxHasher::default();
    h.write(&data);
    let base = format!("{:016x}{:x}", h.finish(), data.len());
    let mut g = PAYLOADS.lock().unwrap();
    let p = g.get_or_insert_with(Default::default);
    let mut key = base.clone();
    let mut k = 0;
    loop {
        match p.get(&key) {
            // the same file included twice is one payload
            Some(x) if **x == data => return key,
            Some(_) => {
                k += 1;
                key = format!("{}_{}", base, k);
            }
            None => {
                p.insert(key.clone(), std::sync::Arc::new(data));
                return key;
            }
        }
    }
}

/// The bytes of a payload marker (`\0relax-include:KEY` after unescaping), if `b` is one.
pub fn payload_of(b: &[u8]) -> Option<std::sync::Arc<Vec<u8>>> {
    let rest = b.strip_prefix(b"\0relax-include:")?;
    let key = std::str::from_utf8(rest).ok()?;
    PAYLOADS.lock().ok()?.as_ref()?.get(key).cloned()
}

// ---------------------------------------------------------------- the built-in table

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Builtin {
    None = 0,
    File,
    Line,
    Column,
    ModulePath,
    Env,
    OptionEnv,
    Stringify,
    Concat,
    IncludeStr,
    IncludeBytes,
    Include,
    /// bitflags 2.x `bitflags!` (bitflags_native.rs); only through `override_rules`
    Bitflags,
    /// std's `thread_local!` (thread_local_mac.rs)
    ThreadLocal,
}

const TABLE: &[(&str, Builtin)] = &[
    ("file", Builtin::File),
    ("line", Builtin::Line),
    ("column", Builtin::Column),
    ("module_path", Builtin::ModulePath),
    ("env", Builtin::Env),
    ("option_env", Builtin::OptionEnv),
    ("stringify", Builtin::Stringify),
    ("concat", Builtin::Concat),
    ("include_str", Builtin::IncludeStr),
    ("include_bytes", Builtin::IncludeBytes),
    ("include", Builtin::Include),
    ("thread_local", Builtin::ThreadLocal),
];

/// Names of every built-in function-like macro (importing them is a no-op).
pub fn names() -> impl Iterator<Item = &'static str> {
    TABLE.iter().map(|x| x.0)
}


pub fn lookup(name: &str) -> Builtin {
    for (n, b) in TABLE {
        if *n == name {
            return *b;
        }
    }
    Builtin::None
}

/// A `macro_rules!` definition Relax replaces with a native expansion: the bitflags crate's
/// `bitflags!` (MAKEPAD_RUN_NATIVE_BITFLAGS=0 keeps the crate's macro_rules, for A/B measurement).
pub fn override_rules(options: &crate::options::Options, d: crate::macros::MacroRef) -> crate::macros::MacroRef {
    if d.name == "bitflags" && d.krate == "bitflags" && options.native_bitflags {
        crate::gen_local! {
            static BF: std::cell::OnceCell<crate::macros::MacroRef>;
        }
        return BF.with(|b| b.get_or_init(|| std::sync::Arc::new(crate::macros::MacroDef { name: "bitflags".to_string(), krate: "bitflags".to_string(), exported: true, arms: Vec::new(), builtin: Builtin::Bitflags as u8, def_file: String::new(), def_pos: 0, input: None })).clone());
    }
    d
}

/// `MacroDef.builtin` ids at and above this are function-like proc macros run by the JIT host
/// (pmhost.rs index).
pub const HOSTED: u8 = 200;

/// Is `name` a built-in macro or one of the graph's proc macros (importing it is a no-op)?
pub fn is_macro_name(name: &str) -> bool {
    lookup(name) != Builtin::None || crate::macro_reg::bang_index(name).is_some() || crate::macro_reg::has_derive(name)
}

/// Built-ins that expand to items (an error there cannot stay with one fn).
pub fn is_item_level(id: u8) -> bool {
    if id >= HOSTED {
        return false;
    }
    matches!(from_u8(id), Builtin::Include | Builtin::Bitflags | Builtin::ThreadLocal)
}

pub fn from_u8(x: u8) -> Builtin {
    if x == Builtin::Bitflags as u8 {
        return Builtin::Bitflags;
    }
    for (_, b) in TABLE {
        if *b as u8 == x {
            return *b;
        }
    }
    Builtin::None
}

crate::gen_local! {
    static DEFS: std::cell::RefCell<Vec<Option<crate::macros::MacroRef>>>;
}

/// The macro definition standing for built-in `name` (no arms; `builtin` set), if any.
pub fn macro_def(name: &str) -> Option<crate::macros::MacroRef> {
    let id = match lookup(name) {
        Builtin::None => match crate::macro_reg::bang_index(name) {
            Some(i) if i < (256 - HOSTED as usize) => HOSTED + i as u8,
            _ => return None,
        },
        b => b as u8,
    };
    DEFS.with(|d| {
        let mut d = d.borrow_mut();
        let i = id as usize;
        if d.len() <= i {
            d.resize(i + 1, None);
        }
        if d[i].is_none() {
            d[i] = Some(std::sync::Arc::new(crate::macros::MacroDef { name: name.to_string(), krate: String::new(), exported: true, arms: Vec::new(), builtin: id, def_file: String::new(), def_pos: 0, input: None }));
        }
        d[i].clone()
    })
}

// ---------------------------------------------------------------- tokens -> proc_macro model

/// The parser's position space: the file's source, then expansion text past its end.
#[derive(Clone, Copy)]
pub struct Buf<'a> {
    pub src: &'a [u8],
    pub ext: SegRef<'a, u8>,
}

impl<'a> Buf<'a> {
    /// A source without expansion text.
    pub fn plain(src: &'a [u8]) -> Self { Self { src, ext: (&[][..]).into() } }
    pub fn get(&self, lo: u32, hi: u32) -> &'a [u8] {
        let base = self.src.len() as u32;
        if lo >= base {
            self.ext.slice((lo - base) as usize, ((hi - base) as usize).min(self.ext.len()))
        } else {
            &self.src[lo as usize..(hi as usize).min(self.src.len())]
        }
    }
}

fn is_punct_kind(k: T) -> bool {
    !matches!(
        k,
        T::Eof
            | T::Ident
            | T::RawBody
            | T::RawIdent
            | T::Lifetime
            | T::Int
            | T::Float
            | T::Str
            | T::ByteStr
            | T::CStr
            | T::RawStr
            | T::Char
            | T::Byte
            | T::OpenParen
            | T::CloseParen
            | T::OpenBracket
            | T::CloseBracket
            | T::OpenBrace
            | T::CloseBrace
    ) && !is_keyword(k)
}

fn is_keyword(k: T) -> bool {
    (k as u8) >= (T::KwAs as u8) && (k as u8) <= (T::KwTry as u8)
}

/// Tokens `toks[lo..hi]` (balanced) as a TokenStream. `None` on unbalanced delimiters.
pub fn to_stream(src: &[u8], toks: &[Tok], lo: usize, hi: usize, span_of: &dyn Fn(u32, u32) -> Span) -> Option<TokenStream> {
    to_stream_buf(Buf::plain(src), toks, lo, hi, span_of)
}

pub fn to_stream_buf(buf: Buf, toks: &(impl TokSeq + ?Sized), lo: usize, hi: usize, span_of: &dyn Fn(u32, u32) -> Span) -> Option<TokenStream> {
    let selected = toks.slice(lo, hi)?;
    let selected = &selected[..];
    let base = buf.src.len() as u32;
    if selected.iter().all(|t| t.hi <= base) {
        return crate::pm_tokens::from_toks(buf.src, selected, span_of).ok();
    }
    if selected.iter().all(|t| t.lo >= base) {
        // the expansion text the tokens spell, as one buffer from `at` (only the span the
        // tokens and their gaps cover is read)
        let nb = buf.ext.base.len() as u32;
        let (text, at) = if selected.iter().all(|t| t.lo - base >= nb) {
            (std::borrow::Cow::Borrowed(buf.ext.own), base + nb)
        } else if selected.iter().all(|t| t.hi - base <= nb) {
            (std::borrow::Cow::Borrowed(buf.ext.base), base)
        } else {
            let lo = selected.iter().map(|t| t.lo).min().unwrap_or(base);
            let hi = selected.iter().map(|t| t.hi).max().unwrap_or(base);
            (buf.ext.window((lo - base) as usize, (hi - base) as usize), lo)
        };
        let shifted: Vec<_> = selected.iter().map(|t| Tok { kind: t.kind, lo: t.lo - at, hi: t.hi - at }).collect();
        return crate::pm_tokens::from_toks(&text, &shifted, &|a,b| span_of(a + at,b + at)).ok();
    }
    // Macro transcription can mix the two backing buffers. Gather only the selected
    // spelling and real same-buffer gaps; the shared builder still owns tree formation.
    let mut text = Vec::new();
    let mut flat = Vec::new();
    let mut positions = Vec::new();
    for (i,t) in selected.iter().enumerate() {
        if t.kind == T::Eof { break; }
        if i > 0 {
            let prev = selected[i-1];
            if prev.hi < t.lo && (prev.hi >= base) == (t.lo >= base) {
                positions.push((text.len() as u32, prev.hi));
                text.extend_from_slice(buf.get(prev.hi,t.lo));
            } else if prev.hi != t.lo { text.push(b' '); }
        }
        let lo = text.len() as u32;
        positions.push((lo,t.lo));
        text.extend_from_slice(buf.get(t.lo,t.hi));
        flat.push(Tok { kind: t.kind, lo, hi: text.len() as u32 });
    }
    let original = |pos: u32, end: bool| {
        let i = positions.partition_point(|(p,_)| if end { *p < pos } else { *p <= pos });
        positions.get(i.saturating_sub(1)).map_or(pos, |(p,o)| o + pos - p)
    };
    crate::pm_tokens::from_toks(&text, &flat, &|a,b| span_of(original(a,false), original(b,true))).ok()
}

/// Source text that lexes back to the same tokens (for re-parsing an expansion).
pub fn render(ts: &TokenStream) -> String {
    let mut out = String::new();
    render_into(&ts.trees, &mut out);
    out
}

fn render_into(trees: &[TokenTree], out: &mut String) {
    for (i, t) in trees.iter().enumerate() {
        match t {
            TokenTree::Group(g) => {
                let (o, c) = match g.delimiter() {
                    Delimiter::Parenthesis => ('(', ')'),
                    Delimiter::Brace => ('{', '}'),
                    Delimiter::Bracket => ('[', ']'),
                    Delimiter::None => (' ', ' '),
                };
                out.push(o);
                render_into(&g.stream().trees, out);
                out.push(c);
            }
            TokenTree::Ident(id) => out.push_str(&id.to_string()),
            TokenTree::Literal(l) => out.push_str(&l.to_string()),
            TokenTree::Punct(p) => out.push(p.as_char()),
        }
        let glue = match (t, trees.get(i + 1)) {
            (_, None) => true,
            (TokenTree::Punct(p), Some(TokenTree::Punct(_))) => p.spacing() == Spacing::Joint,
            // lifetime `'a`, `$name`
            (TokenTree::Punct(p), Some(TokenTree::Ident(_))) => p.as_char() == '\'' || (p.as_char() == '$' && p.spacing() == Spacing::Joint),
            _ => false,
        };
        if !glue {
            out.push(' ');
        }
    }
}

// ---------------------------------------------------------------- stringify!

/// rustc's `stringify!`: tokens printed with source spacing (a space only where the source
/// had whitespace and rustc's `space_between` allows one).
fn stringify(buf: Buf, toks: &(impl TokSeq + ?Sized), lo: usize, hi: usize) -> String {
    stringify_o(buf, toks, lo, hi, Default::default(), u32::MAX)
}

/// stringify! with substituted fragment tokens' spacing taken from their source (rustc keeps a
/// fragment's tokens with their original spans and spacing): `origins` is Parser::tok_origin,
/// `src_len` the length of the file's own text.
fn stringify_o(buf: Buf, toks: &(impl TokSeq + ?Sized), lo: usize, hi: usize, origins: crate::origin_table::OSegRef<crate::origin_table::Maps>, src_len: u32) -> String {
    let orig = |mut pos: u32| -> Option<u32> {
        for _ in 0..32 {
            if pos < src_len {
                return Some(pos);
            }
            match origins.floor_exact(pos).map(|(_, r)| r) {
                Some((t, o, n)) if pos < t + n => pos = o + (pos - t),
                _ => return None,
            }
        }
        None
    };
    let mut out = String::new();
    let mut i = lo;
    while i < hi {
        let t = toks.at(i);
        out.push_str(std::str::from_utf8(buf.get(t.lo, t.hi)).unwrap_or("?"));
        if i + 1 < hi {
            let n = toks.at(i + 1);
            // tokens that stand for source tokens: the source's spacing
            let ws = match (orig(t.lo), orig(n.lo)) {
                (Some(a), Some(b)) if b >= a => b > a + (t.hi - t.lo),
                _ => n.lo > t.hi,
            };
            let tt = buf.get(t.lo, t.hi);
            let nt = buf.get(n.lo, n.hi);
            let is_p = |k: T| is_punct_kind(k);
            let mut space = ws;
            // `.` + non-punct, `$` + ident, non-punct + `,` `;` `.`, ident + `(`, `#` + `[`
            if t.kind == T::Dot && !is_p(n.kind) {
                space = false;
            }
            if t.kind == T::Dollar && matches!(n.kind, T::Ident) {
                space = false;
            }
            if matches!(n.kind, T::Comma | T::Semi | T::Dot) && !is_p(t.kind) && !matches!(t.kind, T::OpenParen | T::OpenBracket | T::OpenBrace) {
                space = false;
            }
            if t.kind == T::Pound && n.kind == T::OpenBracket {
                space = false;
            }
            // paths print without spaces around `::` (a `$p:path` / `$t:ty` fragment is
            // pretty-printed by rustc: `std::option`)
            if t.kind == T::PathSep || n.kind == T::PathSep {
                space = false;
            }
            // rustc prints a brace group as `{ .. }`, other delimiters without inner spaces
            if t.kind == T::OpenBrace && n.kind != T::CloseBrace {
                space = true;
            }
            if n.kind == T::CloseBrace && t.kind != T::OpenBrace {
                space = true;
            }
            if matches!(t.kind, T::OpenParen | T::OpenBracket) || matches!(n.kind, T::CloseParen | T::CloseBracket) {
                space = false;
            }
            let _ = (tt, nt);
            if space {
                out.push(' ');
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------- expansion entry points

/// Authored position of a proc-macro input token. Substitutions retain their
/// fragment spans; literal tokens retain the recorded definition-body spans.
/// This does not change identifier hygiene or the call-site position of line!().
fn src_origin(p: &crate::parser::Parser, mut pos: u32) -> u32 {
    let base = p.src.len() as u32;
    for _ in 0..64 {
        if pos < base {
            return pos;
        }
        match proc_token_origin(p, pos) {
            Some(origin) if origin != pos => pos = origin,
            _ => break,
        }
    }
    orig_pos(p, pos)
}

fn proc_token_origin(p: &crate::parser::Parser, pos: u32) -> Option<u32> {
    for origins in [&p.tok_origin, &p.body_origin] {
        if let Some((_, (t, o, n))) = origins.floor(pos) {
            if pos < t + n { return Some(o + (pos - t)); }
        }
    }
    None
}

/// Map the final byte of this token, rather than extending an unrelated earlier
/// fragment across intervening literal tokens in the expansion.
fn src_origin_end(p: &crate::parser::Parser, z: u32) -> Option<u32> {
    let o = src_origin(p, proc_token_origin(p, z.checked_sub(1)?)?);
    (o < p.src.len() as u32).then_some(o + 1)
}

fn orig_pos(p: &crate::parser::Parser, mut pos: u32) -> u32 {
    let base = p.src.len() as u32;
    for _ in 0..64 {
        if pos < base {
            break;
        }
        match p.exp_map.iter().find(|x| pos >= x.0 && pos <= x.1) {
            Some(x) => pos = x.2,
            None => break,
        }
    }
    pos
}

fn str_lit(s: &str) -> String {
    format!("{:?}", s)
}

/// Value of a string literal token's text (`"..."` or `r#"..."#`).
fn lit_str_value(text: &str) -> Option<String> {
    if text.starts_with('"') {
        Some(crate::lit::unescape_str(text))
    } else if text.starts_with('r') {
        let t = text.trim_start_matches('r');
        let h = t.len() - t.trim_start_matches('#').len();
        let t = &t[h..];
        Some(t.get(1..t.len().checked_sub(1 + h)?)?.to_string())
    } else {
        None
    }
}

/// Expands built-in `b` called with the tokens `toks[lo..hi]` (inside the delimiters);
/// `call_lo..call_hi` is the whole call. Returns the expansion's source text.
/// Expands built-in or hosted macro `id` called with the tokens `toks[lo..hi]` (inside the
/// delimiters); `call_lo..call_hi` is the whole call. Linked proc macros' output tokens are
/// passed on as tokens (no text round trip).
pub fn expand(p: &crate::parser::Parser, id: u8, lo: usize, hi: usize, call_lo: u32, call_hi: u32) -> Result<Expansion, String> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::MacroOutput);
    let key = if id >= HOSTED && crate::macro_reg::reusable_bang((id - HOSTED) as usize) {
        crate::parse_work::key_tokens(p, &format!("hosted-bang-{id}"), call_lo, call_hi, lo..hi, item_location(p, call_lo))
    } else { None }; // env!, include_*! and line! must observe the current inputs
    if let Some(x) = crate::parse_work::get(&key) { return Ok(x); }
    if id >= HOSTED {
        let i = (id - HOSTED) as usize;
        let ts = expand_pm_ts(p, true, lo, hi, call_lo, call_hi, |input, call| crate::macro_reg::run_bang(p.session, i, input, call))?;
        let mut out = Expansion { text: String::new(), toks: Vec::new(), origins: Vec::new() };
        emit_tokens(&ts.trees, p.edition, &mut out, &mut Vec::new())?;
        crate::parse_work::put(key, &out);
        return Ok(out);
    }
    if from_u8(id) == Builtin::Include {
        let (path, data) = include_input(p, lo, hi, call_lo)?;
        let text = String::from_utf8(data).map_err(|_| format!("`{}` is not UTF-8", path))?;
        let sources = p.sources.as_ref().ok_or("include! requires a source registry")?;
        let file = sources.register(&path, std::sync::Arc::from(text.as_bytes()))?;
        let mut toks = Vec::new();
        crate::lexer::lex(text.as_bytes(), p.edition, &mut toks).map_err(|e| format!("`{path}`: {}", e.msg))?;
        toks.retain(|t| t.kind != T::Eof);
        let mut origins = Vec::new();
        let mut origin = |lo, hi| origins.push(TokenOrigin { lo, hi, source: SourceSpan { file, lo, hi } });
        for t in &toks {
            // Proc macros split joint punctuation into smaller source ranges. Lifetimes
            // deliberately keep one full span for both the apostrophe and name, like rustc.
            if t.kind == T::AtId {
                origin(t.lo, t.lo + 1);
                origin(t.lo + 1, t.hi);
            } else if (T::Semi as u8..=T::LArrow as u8).contains(&(t.kind as u8))
                || matches!(t.kind, T::ColonEq | T::PlusColon | T::OtherPunct)
            {
                for (i, ch) in text[t.lo as usize..t.hi as usize].char_indices() {
                    origin(t.lo + i as u32, t.lo + (i + ch.len_utf8()) as u32);
                }
            } else {
                origin(t.lo, t.hi);
            }
        }
        return Ok(Expansion { text, toks, origins });
    }
    Expansion::lexed(expand_text(p, id, lo, hi, call_lo, call_hi)?)
}

impl Expansion {
    /// Flatten a whole file or macro output with one spelling buffer and full token origins.
    pub fn from_stream(stream: &TokenStream, edition: u16) -> Result<Self, String> {
        let mut out = Self { text: String::new(), toks: Vec::new(), origins: Vec::new() };
        emit_tokens(&stream.trees, edition, &mut out, &mut Vec::new())?;
        Ok(out)
    }

    /// Source text of an expansion, lexed.
    pub fn lexed(text: String) -> Result<Expansion, String> {
        let mut toks = Vec::new();
        crate::lexer::lex(text.as_bytes(), 2021, &mut toks).map_err(|e| format!("expands to text that does not lex: {}", e.msg))?;
        toks.retain(|t| t.kind != T::Eof);
        Ok(Expansion { text, toks, origins: Vec::new() })
    }
}

/// Resolve nested includes relative to the file that authored the invocation.
fn include_input(p: &crate::parser::Parser, lo: usize, hi: usize, call_lo: u32) -> Result<(String, Vec<u8>), String> {
    let args = lit_args(p, lo, hi)?;
    let rel = args.first().ok_or("include! takes a path")?;
    let source = p.source_span(call_lo);
    let authored = if source.file != 0 {
        Some(p.sources.as_ref().ok_or("include! requires a source registry")?.get(source.file)?)
    } else { None };
    let current = authored.as_ref().map(|s| s.path.as_str()).or_else(|| p.bctx.as_ref().map(|c| c.path.as_str()));
    let path = if std::path::Path::new(rel).is_absolute() {
        rel.clone()
    } else if let Some(path) = current {
        format!("{}/{}", crate::program::dir_of(path), rel)
    } else { rel.clone() };
    let path = crate::vfs::normalize_path(&path);
    let data = crate::vfs::read(&path).map_err(|e| format!("cannot read `{}`: {} -- check the path (relative to this file)", path, e))?;
    crate::sip::note_late_input(p.bctx.as_ref().map_or("", |c| c.krate.as_str()), crate::sip::LATE_FILE, &path, Some(&data));
    Ok((path, data))
}

fn expand_text(p: &crate::parser::Parser, id: u8, lo: usize, hi: usize, call_lo: u32, call_hi: u32) -> Result<String, String> {
    let b = from_u8(id);
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let toks = &p.toks;
    let ctx = p.bctx.as_deref();
    match b {
        Builtin::File => {
            let source = p.source_span(call_lo);
            if source.file != 0 {
                if let Some(sources) = &p.sources { return Ok(str_lit(&file_macro_path(&sources.get(source.file)?.path))); }
            }
            let f = match ctx {
                Some(c) => c.file_macro.clone(),
                None => String::new(),
            };
            return Ok(str_lit(&f));
        }
        Builtin::Line | Builtin::Column => {
            if let Some(span) = p.input_span(call_lo, call_hi) {
                let value = if b == Builtin::Line { span.start.line } else { span.start.column };
                return Ok(format!("{}u32", value));
            }
            let pos = orig_pos(p, call_lo);
            let lc = match ctx {
                Some(c) => line_col(p.src, c.line_starts(p.src), pos),
                None => line_col(p.src, &line_starts(p.src), pos),
            };
            return Ok(if b == Builtin::Line { format!("{}u32", lc.line) } else { format!("{}u32", lc.column) });
        }
        Builtin::ModulePath => {
            let mut m = match ctx {
                Some(c) => c.module_path.clone(),
                None => p.krate.clone(),
            };
            for s in &p.mod_stack {
                m.push_str("::");
                m.push_str(s);
            }
            return Ok(str_lit(&m));
        }
        Builtin::Stringify => {
            return Ok(str_lit(&stringify_o(buf, toks, lo, hi, p.tok_origin.view(), p.src.len() as u32)));
        }
        Builtin::Env | Builtin::OptionEnv => {
            let args = lit_args(p, lo, hi)?;
            let name = args.first().cloned().ok_or_else(|| "env! takes a variable name -- env!(\"NAME\")".to_string())?;
            let v = env_value(ctx, &name);
            return if b == Builtin::OptionEnv {
                Ok(match v {
                    Some(s) => format!("::core::option::Option::Some({})", str_lit(&s)),
                    None => "::core::option::Option::None::<&'static str>".to_string(),
                })
            } else {
                match v {
                    Some(s) => Ok(str_lit(&s)),
                    None => Err(match args.get(1) {
                        Some(m) => m.clone(),
                        None => format!("environment variable `{}` not defined at compile time -- set it before running makepad run", name),
                    }),
                }
            };
        }
        Builtin::Concat => {
            let parts = concat_args(p, lo, hi)?;
            return Ok(str_lit(&parts.concat()));
        }
        Builtin::ThreadLocal => {
            let mut v: Vec<(T, &[u8])> = Vec::new();
            for i in lo..hi {
                let t = p.toks.at(i);
                v.push((t.kind, p.tok_text(t)));
            }
            return crate::thread_local_mac::expand(&v);
        }
        Builtin::IncludeStr | Builtin::IncludeBytes => {
            let (path, data) = include_input(p, lo, hi, call_lo)?;
            return if b == Builtin::IncludeStr {
                let s = String::from_utf8(data).map_err(|_| format!("`{}` is not UTF-8 -- use include_bytes!", path))?;
                Ok(str_lit(&s))
            } else {
                // the bytes travel as a payload behind a short marker literal (escaping a
                // font into `b"\x.."` text and lexing / unescaping it again cost ~4 bytes of
                // work per byte, three times): lower::unescape_bytes resolves the marker
                Ok(format!("b\"{}{}\"", PAYLOAD_MARK, register_payload(data)))
            };
        }
        _ => {}
    }
    // native bitflags!
    let span_of = |a: u32, z: u32| Span { file: 0, lo: a, hi: z, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 } };
    let input = to_stream_buf(buf, toks, lo, hi, &span_of).ok_or_else(|| "unbalanced delimiters in macro input".to_string())?;
    if b == Builtin::Bitflags {
        let c = if p.krate == "bitflags" { "crate".to_string() } else { "::__relax_crate_bitflags".to_string() };
        return crate::bitflags_native::expand(&input, &c, p.bctx.as_ref().map(|x| &x.cfg));
    }
    let _ = (call_lo, call_hi);
    Err("not a built-in macro".to_string())
}

/// Indexed character columns: count UTF-8 continuation bytes, rather than scanning
/// the entire line prefix at every endpoint (generated lines can be very long).
fn continuation_bytes(src: &[u8]) -> Vec<u32> {
    src.iter().enumerate().filter_map(|(i, b)| (b & 0xc0 == 0x80).then_some(i as u32)).collect()
}

#[derive(Default, Clone, Copy)]
struct ColumnCursor {
    line: usize,
    line_cs: usize,
    pos: u32,
    cs: usize,
}

fn indexed_line_col(src: &[u8], ls: &[u32], cs: &[u32], cursor: &std::cell::Cell<ColumnCursor>, pos: u32) -> LineColumn {
    let pos = pos.min(src.len() as u32);
    let mut c = cursor.get();
    // Adjacent endpoints usually stay on the same line. Group spans revisit their
    // opening position, so jumps in either direction must retain indexed fallback.
    if pos < ls[c.line] || ls.get(c.line + 1).is_some_and(|&next| pos >= next) {
        c.line = ls.partition_point(|&x| x <= pos) - 1;
        c.line_cs = cs.partition_point(|&x| x < ls[c.line]);
    }
    if pos >= c.pos && pos - c.pos <= 256 {
        while cs.get(c.cs).is_some_and(|&x| x < pos) { c.cs += 1; }
    } else {
        c.cs = cs.partition_point(|&x| x < pos);
    }
    c.pos = pos;
    cursor.set(c);
    LineColumn { line: c.line + 1, column: (pos - ls[c.line]) as usize + 1 - (c.cs - c.line_cs) }
}

/// Retain the old, independent line-prefix implementation as the span oracle.
/// This also measures the individual suspects without clock reads per token.
fn legacy_pm_input(p: &crate::parser::Parser, span_needed: bool, lo: usize, hi: usize, call_lo: u32, call_hi: u32, profile: bool) -> (TokenStream, Span) {
    let started = std::time::Instant::now();
    let src = p.src;
    let ext_view = p.ext.view();
    let ext_c = ext_view.contiguous();
    let ext: &[u8] = &ext_c;
    let ls = line_starts(src);
    let ext_ls = line_starts(ext);
    let indexes = started.elapsed();
    let queries = std::cell::RefCell::new(Vec::new());
    let span_of = |a: u32, z: u32| {
        if profile { queries.borrow_mut().push((a, z)); }
        if let Some(span) = p.input_span(a, z) { return span; }
        if !span_needed { return Span { lo: a, hi: z, ..Span::DUMMY }; }
        let base = src.len() as u32;
        if a >= base {
            let o = src_origin(p, a);
            if o < base {
                let e = src_origin_end(p, z).unwrap_or(o).min(base);
                return Span { file: 0, lo: a, hi: z, start: line_col(src, &ls, o), end: line_col(src, &ls, e) };
            }
            Span { file: 0, lo: a, hi: z, start: line_col(ext, &ext_ls, a - base), end: line_col(ext, &ext_ls, z.saturating_sub(base)) }
        } else {
            Span { file: 0, lo: a, hi: z, start: line_col(src, &ls, a), end: line_col(src, &ls, z.min(base)) }
        }
    };
    let input = to_stream_buf(Buf { src, ext: ext_view }, &p.toks, lo, hi, &span_of).expect("legacy macro input");
    let call = span_of(call_lo, call_hi);
    let total = started.elapsed();
    if profile {
        let started = std::time::Instant::now();
        let base = src.len() as u32;
        let mut generated = 0;
        let positions: Vec<_> = queries.into_inner().into_iter().map(|(a, z)| {
            if a >= base {
                generated += 1;
                let o = src_origin(p, a);
                if o < base { (false, o, src_origin_end(p, z).unwrap_or(o).min(base)) }
                else { (true, a - base, z.saturating_sub(base)) }
            } else { (false, a, z.min(base)) }
        }).collect();
        let origins = started.elapsed();
        let started = std::time::Instant::now();
        for &(ext_pos, a, z) in &positions {
            let (text, lines) = if ext_pos { (&ext[..], &ext_ls) } else { (src, &ls) };
            std::hint::black_box((line_col(text, lines, a), line_col(text, lines, z)));
        }
        let columns = started.elapsed();
        let started = std::time::Instant::now();
        std::hint::black_box(to_stream_buf(Buf { src, ext: ext_view }, &p.toks, lo, hi, &|_, _| Span::DUMMY));
        let stream = started.elapsed();
        eprintln!("input-profile: src={} ext={} expansions={} origins={} spans={} generated={} legacy={:.3} indexes={:.3} origins={:.3} line_col={:.3} stream_no_spans={:.3} ms",
            src.len(), ext.len(), p.exp_map.len(), p.tok_origin.len(), positions.len(), generated,
            total.as_secs_f64()*1000., indexes.as_secs_f64()*1000., origins.as_secs_f64()*1000., columns.as_secs_f64()*1000., stream.as_secs_f64()*1000.);
    }
    (input, call)
}

fn verify_pm_spans(actual: &TokenStream, expected: &TokenStream) {
    assert_eq!(actual.trees.len(), expected.trees.len(), "macro input token count");
    for (a, b) in actual.trees.iter().zip(&expected.trees) {
        assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b));
        assert_eq!(a.span(), b.span(), "macro input span (lo, hi, start, end)");
        if let (TokenTree::Group(a), TokenTree::Group(b)) = (a, b) {
            verify_pm_spans(&a.stream, &b.stream);
        }
    }
}

/// A function-like proc macro's output tokens for the call `toks[lo..hi]` (`run`: the JIT
/// host); `span_needed`: the input's spans carry line/column.
fn expand_pm_ts(p: &crate::parser::Parser, span_needed: bool, lo: usize, hi: usize, call_lo: u32, call_hi: u32, run: impl FnOnce(TokenStream, Span) -> Result<TokenStream, String>) -> Result<TokenStream, String> {
    let input_timer = crate::parse_work::time(7);
    let index_timer = crate::parse_work::time(8);
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let ctx = p.bctx.as_deref();
    let ls_own;
    let ls: &[u32] = if span_needed {
        match ctx {
            Some(c) => c.line_starts(p.src),
            None => {
                ls_own = line_starts(p.src);
                &ls_own
            }
        }
    } else {
        &[]
    };
    let src: &[u8] = p.src;
    let cs_own;
    let cs: &[u32] = if span_needed {
        match ctx {
            Some(c) => c.continuations(src),
            None => {
                cs_own = continuation_bytes(src);
                &cs_own
            }
        }
    } else { &[] };
    // Most calls use source tokens, or expansions mapped back to source. Do not
    // scan the accumulated expansion buffer unless an unmapped token needs it.
    let ext_index = std::cell::OnceCell::new();
    let src_cursor = std::cell::Cell::new(ColumnCursor::default());
    let ext_cursor = std::cell::Cell::new(ColumnCursor::default());
    drop(index_timer);
    let stream_timer = crate::parse_work::time(9);
    let span_of = |a: u32, z: u32| -> Span {
        if let Some(span) = p.input_span(a, z) { return span; }
        if !span_needed {
            return Span { file: 0, lo: a, hi: z, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 } };
        }
        let base = src.len() as u32;
        if a >= base {
            // a token a macro_rules expansion substituted from a fragment keeps the fragment
            // token's position (rustc: substituted tokens keep their spans); other expansion
            // tokens are placed at the outermost call site (like `line!()`)
            let o = src_origin(p, a);
            if o < base {
                // the end: after the last substituted token before `z` (a verbatim copy)
                let e = src_origin_end(p, z).unwrap_or(o).min(base);
                return Span { file: 0, lo: a, hi: z, start: indexed_line_col(src, ls, cs, &src_cursor, o), end: indexed_line_col(src, ls, cs, &src_cursor, e) };
            }
            let (ext, ext_ls, ext_cs) = ext_index.get_or_init(|| {
                let ext = p.ext.view().contiguous();
                let (ls, cs) = (line_starts(&ext), continuation_bytes(&ext));
                (ext, ls, cs)
            });
            Span { file: 0, lo: a, hi: z, start: indexed_line_col(ext, ext_ls, ext_cs, &ext_cursor, a - base), end: indexed_line_col(ext, ext_ls, ext_cs, &ext_cursor, z.saturating_sub(base)) }
        } else {
            Span { file: 0, lo: a, hi: z, start: indexed_line_col(src, ls, cs, &src_cursor, a), end: indexed_line_col(src, ls, cs, &src_cursor, z.min(base)) }
        }
    };
    let input = to_stream_buf(buf, &p.toks, lo, hi, &span_of).ok_or_else(|| "unbalanced delimiters in macro input".to_string())?;
    let call = span_of(call_lo, call_hi);
    drop(stream_timer);
    drop(input_timer);
    let verify = std::env::var("MAKEPAD_RUN_PARSE_VERIFY").as_deref() == Ok("1");
    let profile = std::env::var("MAKEPAD_RUN_PARSE_INPUT_PROFILE").as_deref() == Ok("1");
    if verify || profile {
        let (expected, expected_call) = legacy_pm_input(p, span_needed, lo, hi, call_lo, call_hi, profile);
        verify_pm_spans(&input, &expected);
        assert_eq!(call, expected_call, "macro call span");
    }
    run(input, call)
}

fn env_value(ctx: Option<&FileCtx>, name: &str) -> Option<String> {
    let v = env_value_inner(ctx, name);
    // a macro input outside the source: part of the crate's cache validity (stable.rs ledger)
    crate::sip::note_late_input(ctx.map_or("", |c| c.krate.as_str()), crate::sip::LATE_ENV, name, v.as_deref().map(|x| x.as_bytes()));
    v
}

fn env_value_inner(ctx: Option<&FileCtx>, name: &str) -> Option<String> {
    // cargo's per-crate environment as `makepad run` registered it (unitgraph.rs), then what Relax
    // knows from the crate's Cargo.toml, then the process environment
    if let Some(c) = ctx {
        if let Some(v) = crate::crates::crate_env_registered(&c.krate, name) {
            return Some(v);
        }
        match name {
            "CARGO_MANIFEST_DIR" => return Some(c.manifest_dir.clone()),
            "CARGO_PKG_NAME" => return Some(c.pkg_name.clone()),
            "CARGO_PKG_VERSION" => return Some(c.pkg_version.clone()),
            "CARGO_CRATE_NAME" => return Some(c.krate.clone()),
            _ => {}
        }
    }
    std::env::var(name).ok()
}

/// The value of tokens [a, z) when they are a literal or a literal-producing built-in call
/// (concat!, stringify!, env!, include_str!, ..); second: whether it is a string.
pub fn eval_lit_range(p: &crate::parser::Parser, a: usize, z: usize) -> Result<(String, bool), String> {
    eval_lit(p, a, z)
}

/// String-literal arguments (`"a", "b"`), nested built-ins expanded.
fn lit_args(p: &crate::parser::Parser, lo: usize, hi: usize) -> Result<Vec<String>, String> {
    let mut v = Vec::new();
    for (a, z) in split_commas(&p.toks, lo, hi) {
        v.push(eval_lit(p, a, z)?.0);
    }
    Ok(v)
}

/// concat!'s pieces: literals as rustc prints them in concat! (strings unquoted, numbers,
/// bools, chars), nested built-ins expanded.
fn concat_args(p: &crate::parser::Parser, lo: usize, hi: usize) -> Result<Vec<String>, String> {
    let mut v = Vec::new();
    for (a, z) in split_commas(&p.toks, lo, hi) {
        v.push(eval_lit(p, a, z)?.0);
    }
    Ok(v)
}

fn split_commas(toks: &(impl TokSeq + ?Sized), lo: usize, hi: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut depth = 0i32;
    let mut s = lo;
    for i in lo..hi {
        match toks.at(i).kind {
            T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
            T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
            T::Comma if depth == 0 => {
                if i > s {
                    v.push((s, i));
                }
                s = i + 1;
            }
            _ => {}
        }
    }
    if hi > s {
        v.push((s, hi));
    }
    v
}

/// The value of a literal-producing argument: a literal, `-`literal, `true`/`false`, or a
/// nested built-in call (`env!(..)`, `stringify!(..)`, ...). Second: was it a string.
fn eval_lit(p: &crate::parser::Parser, a: usize, z: usize) -> Result<(String, bool), String> {
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let toks = &p.toks;
    let text = |i: usize| String::from_utf8_lossy(buf.get(toks.at(i).lo, toks.at(i).hi)).into_owned();
    if z == a + 1 {
        let t = toks.at(a);
        let s = text(a);
        return match t.kind {
            T::Str | T::RawStr => Ok((lit_str_value(&s).unwrap_or_default(), true)),
            T::Char => Ok((crate::lit::unescape_str(&format!("\"{}\"", &s[1..s.len() - 1])), false)),
            T::Int | T::Float => Ok((num_lit_value(&s, t.kind == T::Int), false)),
            T::KwTrue | T::KwFalse => Ok((s, false)),
            _ => Err(format!("expected a literal, found `{}`", s)),
        };
    }
    if z == a + 2 && toks.at(a).kind == T::Minus && matches!(toks.at(a + 1).kind, T::Int | T::Float) {
        return Ok((format!("-{}", num_lit_value(&text(a + 1), toks.at(a + 1).kind == T::Int)), false));
    }
    // nested macro call: path ! ( .. )
    let mut i = a;
    while i < z && matches!(toks.at(i).kind, T::Ident | T::PathSep | T::KwCrate | T::Dollar) {
        i += 1;
    }
    if i > a && i + 2 < z + 1 && toks.at(i).kind == T::Not && matches!(toks.at(i + 1).kind, T::OpenParen | T::OpenBracket | T::OpenBrace) {
        let name = text(i - 1);
        let b = lookup(&name);
        if b != Builtin::None && z >= i + 3 {
            let out = expand_text(p, b as u8, i + 2, z - 1, toks.at(a).lo, toks.at(z - 1).hi)?;
            let mut lt = Vec::new();
            crate::lexer::lex(out.as_bytes(), 2021, &mut lt).map_err(|_| "nested macro output does not lex".to_string())?;
            if lt.len() >= 1 {
                let s = &out[lt[0].lo as usize..lt[0].hi as usize];
                return match lt[0].kind {
                    T::Str | T::RawStr => Ok((lit_str_value(s).unwrap_or_default(), true)),
                    T::Int | T::Float => Ok((num_lit_value(s, lt[0].kind == T::Int), false)),
                    _ => Ok((s.to_string(), false)),
                };
            }
        }
    }
    Err(format!("expected a literal, found `{}`", stringify(buf, toks, a, z)))
}

/// A number literal as concat! writes it (rustc concat.rs over LitKind): an integer by its value
/// in decimal (`0x1b` -> "27", `1_000u32` -> "1000"), a float as written without its suffix and
/// underscores (`1_0.5e3f64` -> "10.5e3").
fn num_lit_value(s: &str, int: bool) -> String {
    let t = strip_num_suffix(s).replace('_', "");
    if !int {
        return t;
    }
    let (radix, digits) = match t.get(..2) {
        Some("0x") => (16, &t[2..]),
        Some("0o") => (8, &t[2..]),
        Some("0b") => (2, &t[2..]),
        _ => (10, &t[..]),
    };
    match u128::from_str_radix(digits, radix) {
        Ok(v) => v.to_string(),
        Err(_) => t,
    }
}

fn strip_num_suffix(s: &str) -> String {
    for suf in ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"] {
        if let Some(x) = s.strip_suffix(suf) {
            if !s.starts_with("0x") || suf.starts_with('u') || suf.starts_with('i') {
                return x.to_string();
            }
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------- derives

/// A derive expansion as tokens over its own text (positions relative to the text, no Eof).
#[derive(Clone)]
pub struct Expansion {
    pub text: String,
    pub toks: Vec<Tok>,
    /// Every output token's source span, sorted by its spelling-buffer offset.
    pub origins: Vec<TokenOrigin>,
}

enum ItemOut {
    /// no derive to run (none named, or the item is cfg-disabled)
    Nothing,
    Done(Expansion),
}

fn trace_on(what: &str) -> bool {
    crate::preload::relax_trace().map_or(false, |t| t.split(',').any(|x| x == what))
}

fn item_location(p: &crate::parser::Parser, mut origin: u32) -> (usize,usize) {
    for _ in 0..64 {
        if origin < p.src.len() as u32 { break; }
        match p.exp_map.iter_rev().find(|(lo,hi,_)| *lo <= origin && origin <= *hi) {
            Some((_,_,at)) if at != origin => origin = at,
            _ => break,
        }
    }
    if origin >= p.src.len() as u32 {
        if let Some(span) = p.input_span(origin, origin + 1) { return (span.start.line as usize, span.start.column as usize); }
    }
    crate::lexer::line_col(p.src,origin)
}

/// Derives of one item, from its source bytes (`item_lo`: its position, for diagnostics).
/// Linked derives run natively, the rest through the JIT host (pmhost.rs; any thread).
fn expand_item(sess: &std::sync::Arc<crate::Session>, path: &str, item: &[u8], item_lo: u32, location: (usize, usize), cfg: Option<&CfgSet>, edition: u16, authored: Option<&dyn Fn(u32, u32) -> Option<Span>>) -> Result<ItemOut, (u32, String)> {
    let span_of = |a: u32, z: u32| authored.and_then(|f| f(item_lo + a, item_lo + z)).unwrap_or(Span { file: 0, lo: item_lo + a, hi: item_lo + z, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 } });
    // lexed from the item's own text: parsing rewrites tokens in place (`>>` closing two
    // generic lists becomes `>`), so the parser's token list no longer spells the source
    let mut fresh = Vec::new();
    if crate::lexer::lex(item, 2021, &mut fresh).is_err() {
        return Err((item_lo, "derive input does not lex".to_string()));
    }
    let input = match to_stream_buf(Buf::plain(item), &fresh, 0, fresh.len(), &span_of) {
        Some(s) => s,
        None => return Err((item_lo, "unbalanced delimiters in derive input".to_string())),
    };
    let (names, item_ts) = prepare_derive_input(input, cfg);
    // a `#[cfg]`-disabled item is gone before derives run (rustc strips it first)
    if names.is_empty() || !item_cfg_on(&item_ts.trees, cfg) {
        return Ok(ItemOut::Nothing);
    }
    if trace_on("pmhost") {
        eprintln!("pmhost: derive({}) on `{}`", names.join(", "), item_ts);
    }
    let call = span_of(0, item.len() as u32);
    // leading space: expansion ranges map to call sites by an inclusive range check, so two
    // expansions must not share an endpoint
    let mut out = Expansion { text: String::from(" "), toks: Vec::new(), origins: Vec::new() };
    let mut scratch = Vec::new();
    for name in names {
        // The host selects a source-matched linked library or the sandboxed JIT.
        let r = match crate::macro_reg::run_derive(sess, &name, &item_ts, call) {
            Some(r) => r,
            // no host for this derive: leave the item as it is
            None => return Ok(ItemOut::Nothing),
        };
        let ts = match r {
            Ok(ts) => ts,
            // the error stays at the item (`compile_error!` item); the file still loads
            Err(m) => {
                if trace_on("pmhost") {
                    eprintln!("pmhost: derive({}) failed: {}", name, m);
                }
                let (line,col) = location;
                crate::macro_reg::derive_failed(path, item_lo, line as u32,col as u32, &name, &m);
                let e = format!("compile_error!({:?});", format!("derive({}): {}", name, m));
                let mut et = Vec::new();
                let _ = crate::lexer::lex(e.as_bytes(), 2021, &mut et);
                emit_lexed(&e, &et, call.into(), &mut out);
                continue;
            }
        };
        let t0 = out.text.len();
        emit_tokens(&ts.trees, edition, &mut out, &mut scratch).map_err(|m| (item_lo, m))?;
        if trace_on("expand:derive") {
            eprintln!("expand derive({}) @{} => {}", name, path, &out.text[t0..]);
        }
        out.text.push('\n');
    }
    Ok(ItemOut::Done(out))
}

/// Appends lexed `text` (its tokens `toks`, Eof dropped) to an expansion.
fn emit_lexed(text: &str, toks: &[Tok], source: SourceSpan, out: &mut Expansion) {
    let base = out.text.len() as u32;
    out.text.push_str(text);
    for t in toks {
        if t.kind != T::Eof {
            out.toks.push(Tok { kind: t.kind, lo: t.lo + base, hi: t.hi + base });
            out.origins.push(TokenOrigin { lo: t.lo + base, hi: t.hi + base, source });
        }
    }
    out.text.push(' ');
}

/// Proc-macro output trees straight to Relax tokens (and the text they point into): idents,
/// groups and single punctuation are classified directly; literals and multi-char
/// punctuation runs (Joint spacing, e.g. `::`, `->`, `..=`) by the lexer's own rules.
fn emit_tokens(trees: &[TokenTree], edition: u16, out: &mut Expansion, scratch: &mut Vec<Tok>) -> Result<(), String> {
    let mut i = 0;
    while i < trees.len() {
        match &trees[i] {
            TokenTree::Group(g) => {
                let (o, c, ko, kc) = match g.delimiter() {
                    Delimiter::Parenthesis => ("(", ")", T::OpenParen, T::CloseParen),
                    Delimiter::Brace => ("{", "}", T::OpenBrace, T::CloseBrace),
                    Delimiter::Bracket => ("[", "]", T::OpenBracket, T::CloseBracket),
                    Delimiter::None => ("", "", T::Eof, T::Eof),
                };
                if ko != T::Eof {
                    push_tok(out, o, ko, g.span.into());
                }
                emit_tokens(&g.stream.trees, edition, out, scratch)?;
                if kc != T::Eof {
                    push_tok(out, c, kc, g.span.into());
                }
            }
            TokenTree::Ident(id) => {
                let lo = out.text.len() as u32;
                let kind = if id.raw {
                    out.text.push_str("r#");
                    T::RawIdent
                } else {
                    crate::lexer::keyword(id.name.as_bytes(), edition).unwrap_or(T::Ident)
                };
                out.text.push_str(&id.name);
                out.toks.push(Tok { kind, lo, hi: out.text.len() as u32 });
                out.origins.push(TokenOrigin { lo, hi: out.text.len() as u32, source: id.span.into() });
                out.text.push(' ');
            }
            TokenTree::Literal(l) => {
                scratch.clear();
                crate::lexer::lex(l.text.as_bytes(), edition, scratch).map_err(|e| format!("invalid token literal: {}", e.msg))?;
                emit_lexed(&l.text, scratch, l.span.into(), out);
            }
            TokenTree::Punct(p) => {
                // lifetime: `'` (Joint) + ident
                if p.ch == '\'' {
                    if let Some(TokenTree::Ident(id)) = trees.get(i + 1) {
                        let lo = out.text.len() as u32;
                        out.text.push('\'');
                        out.text.push_str(&id.name);
                        out.toks.push(Tok { kind: T::Lifetime, lo, hi: out.text.len() as u32 });
                        let first = SourceSpan::from(p.span);
                        let last = SourceSpan::from(id.span);
                        if first.file == last.file {
                            out.origins.push(TokenOrigin { lo, hi: out.text.len() as u32, source: first.join(last) });
                        } else {
                            out.origins.push(TokenOrigin { lo, hi: lo + 1, source: first });
                            out.origins.push(TokenOrigin { lo: lo + 1, hi: out.text.len() as u32, source: last });
                        }
                        out.text.push(' ');
                        i += 2;
                        continue;
                    }
                }
                // a run of Joint puncts ending in the next one
                let mut j = i;
                while j + 1 < trees.len() {
                    match (&trees[j], &trees[j + 1]) {
                        (TokenTree::Punct(a), TokenTree::Punct(b)) if a.spacing == Spacing::Joint && b.ch != '\'' => j += 1,
                        _ => break,
                    }
                }
                // `#` never forms a compound Rust token. Lexing a Joint `#!`
                // fragment alone would mistake an inner attribute for a shebang.
                if j == i || p.ch == '#' {
                    if let Some(k) = single_punct(p.ch) {
                        let mut b = [0u8; 4];
                        push_tok(out, p.ch.encode_utf8(&mut b), k, p.span.into());
                        i += 1;
                        continue;
                    }
                }
                let mut run = String::new();
                let mut spans = Vec::new();
                for t in &trees[i..=j] {
                    if let TokenTree::Punct(q) = t {
                        let lo = run.len() as u32;
                        run.push(q.ch);
                        spans.push(TokenOrigin { lo, hi: run.len() as u32, source: q.span.into() });
                    }
                }
                scratch.clear();
                crate::lexer::lex(run.as_bytes(), edition, scratch).map_err(|e| format!("invalid punctuation: {}", e.msg))?;
                let base = out.text.len() as u32;
                out.text.push_str(&run);
                for t in scratch.iter().filter(|t| t.kind != T::Eof) {
                    out.toks.push(Tok { kind: t.kind, lo: base + t.lo, hi: base + t.hi });
                    let mut parts = spans.iter().filter(|s| s.lo < t.hi && t.lo < s.hi);
                    let first = *parts.next().expect("punctuation has an input span");
                    let mut origin = TokenOrigin { lo: base + t.lo, hi: base + first.hi.min(t.hi), source: first.source };
                    for part in parts {
                        if part.source.file == origin.source.file {
                            origin.hi = base + part.hi.min(t.hi);
                            origin.source = origin.source.join(part.source);
                        } else {
                            out.origins.push(origin);
                            origin = part.shifted(base);
                        }
                    }
                    out.origins.push(origin);
                }
                out.text.push(' ');
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    Ok(())
}

fn push_tok(out: &mut Expansion, s: &str, kind: T, source: SourceSpan) {
    let lo = out.text.len() as u32;
    out.text.push_str(s);
    out.toks.push(Tok { kind, lo, hi: out.text.len() as u32 });
    out.origins.push(TokenOrigin { lo, hi: out.text.len() as u32, source });
    out.text.push(' ');
}

fn single_punct(c: char) -> Option<T> {
    Some(match c {
        ',' => T::Comma,
        ';' => T::Semi,
        ':' => T::Colon,
        '.' => T::Dot,
        '#' => T::Pound,
        '$' => T::Dollar,
        '?' => T::Question,
        '@' => T::At,
        '~' => T::Tilde,
        '=' => T::Eq,
        '<' => T::Lt,
        '>' => T::Gt,
        '+' => T::Plus,
        '-' => T::Minus,
        '*' => T::Star,
        '/' => T::Slash,
        '%' => T::Percent,
        '^' => T::Caret,
        '!' => T::Not,
        '&' => T::And,
        '|' => T::Or,
        _ => return None,
    })
}

crate::gen_local! {
    /// derive expansions computed ahead in parallel (pre_expand_derives): (file, item byte range)
    static PRE: std::cell::RefCell<crate::fxhash::HashMap<(String, u32, u32), Result<Option<Expansion>, (u32, String)>>>;
}


/// For an item `toks[lo..hi]` (outer attributes included) whose attributes name a derive Relax
/// runs: the derives' expansion as tokens. `Ok(None)`: nothing to expand.
pub fn derive_items(p: &crate::parser::Parser, attrs: &[crate::ast::Attr], lo: usize, hi: usize) -> Result<Option<Expansion>, (u32, String)> {
    crate::preload::timed(1, || derive_items_inner(p, attrs, lo, hi))
}

fn derive_items_inner(p: &crate::parser::Parser, attrs: &[crate::ast::Attr], lo: usize, hi: usize) -> Result<Option<Expansion>, (u32, String)> {
    let sess = p.session;
    // MAKEPAD_RUN_DERIVES=0: derives are not expanded at all (baseline for measurements only)
    if crate::preload::derives_off(&p.session.options) {
        return Ok(None);
    }
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let toks = &p.toks;
    let item_lo = toks.at(lo).lo;
    let item_hi = toks.at(hi.saturating_sub(1)).hi;
    // computed ahead on the worker threads?
    if let Some(c) = p.bctx.as_ref().filter(|_| !crate::macro_reg::declaring()) {
        if let Some(r) = PRE.with(|m| m.borrow_mut().remove(&(c.path.clone(), item_lo, item_hi))) {
            crate::macro_reg::add_pre(true);
            return r;
        }
    }
    // quick check: any attribute containing one of our derive names
    let mut any = false;
    let mut reusable = true;
    for a in attrs {
        let (alo, ahi) = (a.toks.lo as usize, a.toks.hi as usize);
        if ahi <= alo {
            continue;
        }
        let first = buf.get(toks.at(alo).lo, toks.at(alo).hi);
        if first != b"derive" && first != b"cfg_attr" {
            continue;
        }
        for i in alo..ahi {
            if toks.at(i).kind == T::Ident {
                let w = buf.get(toks.at(i).lo, toks.at(i).hi);
                if std::str::from_utf8(w).map_or(false, is_derive) {
                    any = true;
                    reusable &= crate::macro_reg::reusable_derive(std::str::from_utf8(w).unwrap());
                }
            }
        }
    }
    if !any {
        return Ok(None);
    }
    let _profile = crate::load_profile::span(p.session.runtime.profile, "derive", p.bctx.as_ref().map_or("", |c| c.path.as_str()));
    let key = if reusable && !crate::macro_reg::declaring() { crate::parse_work::key_tokens(p, "hosted-derive", item_lo, item_hi, lo..hi, item_location(p, item_lo)) } else { None };
    let reused = crate::parse_work::get(&key);
    // MAKEPAD_RUN_REUSE_VERIFY=1: every reused derive expansion is recomputed and must be identical.
    if reused.is_some() && std::env::var_os("MAKEPAD_RUN_REUSE_VERIFY").is_none() { return Ok(reused); }
    crate::macro_reg::add_pre(false);
    let cfg = p.bctx.as_ref().map(|c| &c.cfg);
    let r = expand_item(sess, p.bctx.as_ref().map_or("", |c| c.path.as_str()), buf.get(item_lo, item_hi), item_lo, item_location(p,item_lo), cfg, p.edition, Some(&|lo, hi| p.input_span(lo, hi)));
    match r? {
        ItemOut::Done(x) => {
            if let Some(reused) = reused {
                assert!(reused.text == x.text && format!("{:?}{:?}", reused.toks, reused.origins) == format!("{:?}{:?}", x.toks, x.origins),
                    "MAKEPAD_RUN_REUSE_VERIFY: reused derive expansion differs at {item_lo}");
                return Ok(Some(reused));
            }
            if !x.text.contains("compile_error") { crate::parse_work::put(key, &x); }
            Ok(Some(x))
        }
        _ => Ok(None),
    }
}

/// The attribute macro on an item (`attrs`, as parsed): the index of the first attribute naming
/// a registered `#[proc_macro_attribute]` (last path segment) that is not a helper attribute of
/// one of the item's derives (rustc: an attribute in scope as both would be ambiguous; makepad's
/// derives own many plain names). Only attributes written as `#[..]` (not cfg_attr output).
pub fn attr_macro_at(p: &crate::parser::Parser, attrs: &[crate::ast::Attr]) -> Option<usize> {
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let toks = &p.toks;
    let text = |i: usize| std::str::from_utf8(buf.get(toks.at(i).lo, toks.at(i).hi)).unwrap_or("").trim_start_matches("r#").to_string();
    let mut derives: Vec<String> = Vec::new();
    let mut cand: Option<(usize, String)> = None;
    for (k, a) in attrs.iter().enumerate() {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        if hi <= lo || hi > toks.len() {
            continue;
        }
        let first = text(lo);
        if first == "derive" {
            for i in lo + 1..hi {
                if toks.at(i).kind == T::Ident {
                    derives.push(text(i));
                }
            }
            continue;
        }
        if cand.is_some() || lo < 2 || toks.at(lo - 2).kind != T::Pound || toks.at(lo - 1).kind != T::OpenBracket || toks.get(hi).map(|t| t.kind) != Some(T::CloseBracket) {
            continue;
        }
        // the path: idents joined by `::`
        let mut i = lo;
        let mut last = String::new();
        while i < hi {
            match toks.at(i).kind {
                T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper => last = text(i),
                T::PathSep => {}
                _ => break,
            }
            i += 1;
        }
        // `#[relax(O2)]` / `#[relax(O3)]` / `#[relax(O3, always)]` (makepad-relax): read by Relax
        // itself from the item's attributes (relax_level), never expanded; other arguments run
        // the macro, whose compile_error! reports them
        if last == "relax" && relax_level_toks(&text, toks, i, hi).is_some() {
            continue;
        }
        if !last.is_empty() && crate::macro_reg::has_attr(&last) {
            cand = Some((k, last));
        }
    }
    let (k, name) = cand?;
    if crate::macro_reg::is_derive_helper(&name, &derives) {
        return None;
    }
    Some(k)
}

/// Optimisation level of a `relax(..)` attribute's arguments at tokens [i, hi) (i at the `(`):
/// 2 = `(O2)`, 3 = `(O3)`, 4 = `(O3, always)`; None for anything else.
pub fn relax_level_toks(text: &dyn Fn(usize) -> String, toks: &(impl TokSeq + ?Sized), i: usize, hi: usize) -> Option<u8> {
    if i >= hi || toks.at(i).kind != T::OpenParen {
        return None;
    }
    let args: Vec<String> = (i + 1..hi).map(|k| text(k)).collect();
    match args.iter().map(|a| a.as_str()).collect::<Vec<_>>().as_slice() {
        ["O2", ")"] => Some(2),
        ["O3", ")"] => Some(3),
        ["O3", ",", "always", ")"] => Some(4),
        _ => None,
    }
}

/// Runs the attribute macro of attribute `attr` on the item at tokens [lo, hi) (its own
/// attributes included, that one removed): its output replaces the item. The arguments are the
/// tokens of the attribute's delimited group after the path (none for a bare `#[name]`). A
/// failed expansion leaves a `compile_error!` item and is a reported failure (derive_failed).
pub fn expand_attr_macro(p: &crate::parser::Parser, attr: &crate::ast::Attr, lo: usize, hi: usize) -> Result<Expansion, (u32, String)> {
    let _timing = crate::parse_work::time(3);
    let buf = Buf { src: p.src, ext: p.ext.view() };
    let toks = &p.toks;
    let (alo, ahi) = (attr.toks.lo as usize, attr.toks.hi as usize);
    let item_lo = toks.at(lo).lo;
    let item_hi = toks.at(hi.saturating_sub(1)).hi;
    let cut_lo = toks.at(alo - 2).lo;
    let cut_hi = toks.at(ahi).hi;
    let path = p.bctx.as_ref().map_or("", |c| c.path.as_str());
    // the attribute's name and arguments
    let mut i = alo;
    let mut name = String::new();
    while i < ahi && matches!(toks.at(i).kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper | T::PathSep) {
        if toks.at(i).kind != T::PathSep {
            name = String::from_utf8_lossy(buf.get(toks.at(i).lo, toks.at(i).hi)).trim_start_matches("r#").to_string();
        }
        i += 1;
    }
    let args: Vec<u8> = if i < ahi && matches!(toks.at(i).kind, T::OpenParen | T::OpenBracket | T::OpenBrace) && ahi >= i + 2 {
        buf.get(toks.at(i + 1).lo.min(toks.at(ahi - 1).lo), toks.at(ahi - 1).lo).to_vec()
    } else {
        Vec::new()
    };
    // a pure attribute macro's expansion is a function of the item's tokens and spans
    // (the derive reuse rule; the key covers the attribute, its arguments and the item)
    let key = if crate::macro_reg::reusable_attr(&name) {
        crate::parse_work::key_tokens(p, &format!("hosted-attr-{name}"), item_lo, item_hi, lo..hi, item_location(p, item_lo))
    } else { None };
    let reused = crate::parse_work::get(&key);
    // MAKEPAD_RUN_REUSE_VERIFY=1: every reused expansion is recomputed and must be identical.
    if reused.is_some() && std::env::var_os("MAKEPAD_RUN_REUSE_VERIFY").is_none() { return Ok(reused.unwrap()); }
    // the item without that attribute
    let mut item: Vec<u8> = buf.get(item_lo, cut_lo).to_vec();
    item.push(b' ');
    item.extend_from_slice(buf.get(cut_hi, item_hi));
    let span_of = |a: u32, z: u32| p.input_span(a, z).unwrap_or(Span { file: 0, lo: a, hi: z, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 } });
    let stream = |text: &[u8], offset: &dyn Fn(u32) -> u32| -> Result<TokenStream, (u32, String)> {
        let mut t = Vec::new();
        if crate::lexer::lex(text, 2021, &mut t).is_err() {
            return Err((item_lo, format!("#[{}] input does not lex", name)));
        }
        to_stream_buf(Buf::plain(text), &t, 0, t.len(), &|a, z| span_of(offset(a), offset(z))).ok_or_else(|| (item_lo, format!("unbalanced delimiters in #[{}] input", name)))
    };
    let args_lo = if i + 1 < ahi { toks.at(i + 1).lo.min(toks.at(ahi - 1).lo) } else { item_lo };
    let attr_ts = stream(&args, &|a| args_lo + a)?;
    let before = cut_lo - item_lo;
    let item_ts = stream(&item, &|a| if a <= before { item_lo + a } else { cut_hi + a - before - 1 })?;
    let call = span_of(item_lo, item_hi);
    let mut out = Expansion { text: String::from(" "), toks: Vec::new(), origins: Vec::new() };
    match crate::macro_reg::run_attr(p.session, &name, &attr_ts, &item_ts, call) {
        Ok(ts) => {
            emit_tokens(&ts.trees, p.edition, &mut out, &mut Vec::new()).map_err(|m| (item_lo, m))?;
            if trace_on("expand:attr") {
                eprintln!("expand #[{}] @{} => {}", name, path, out.text);
            }
            out.text.push('\n');
            if let Some(reused) = reused {
                assert!(reused.text == out.text && format!("{:?}{:?}", reused.toks, reused.origins) == format!("{:?}{:?}", out.toks, out.origins),
                    "MAKEPAD_RUN_REUSE_VERIFY: reused #[{}] expansion differs at {item_lo}", name);
            }
            crate::parse_work::put(key, &out);
            return Ok(out);
        }
        Err(m) => {
            let (line,col) = item_location(p,item_lo);
            crate::macro_reg::derive_failed(path, item_lo,line as u32,col as u32, &format!("#[{}]", name), &m);
            let e = format!("compile_error!({:?});", format!("#[{}]: {}", name, m));
            let mut et = Vec::new();
            let _ = crate::lexer::lex(e.as_bytes(), 2021, &mut et);
            emit_lexed(&e, &et, call.into(), &mut out);
        }
    }
    out.text.push('\n');
    Ok(out)
}

/// Expands the derives of a crate's files ahead of parsing, in parallel (the parser takes
/// the results by item position; anything missed is expanded when parsed). `files`: (path,
/// source, tokens) as the macro prescan lexed them.
pub fn pre_expand_derives(files: &[(&str, &[u8], &[Tok])], cfg: &CfgSet, edition: u16) {
    let sess = &cfg.session;
    if !cfg.session.options.derives || !cfg.session.options.pre_derives {
        return;
    }
    // sites: (file, item lo, item hi)
    let mut sites: Vec<(usize, u32, u32)> = Vec::new();
    for (fi, (_, src, toks)) in files.iter().enumerate() {
        derive_sites(src, toks, |lo, hi| sites.push((fi, lo, hi)));
    }
    if sites.is_empty() {
        return;
    }
    // line starts of the files with sites, once (each site's line/column)
    let mut lines: Vec<Vec<u32>> = vec![Vec::new(); files.len()];
    for &(fi, _, _) in &sites {
        if lines[fi].is_empty() {
            lines[fi] = crate::lexer::line_starts(files[fi].1);
        }
    }
    let t_wall = std::time::Instant::now();
    let jobs = crate::preload::jobs();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<Vec<(usize, Result<ItemOut, (u32, String)>)>> = std::thread::scope(|sc| {
        let mut hs = Vec::new();
        for _ in 0..jobs.min(sites.len()) {
            hs.push(sc.spawn(|| { let _z = crate::zone_alloc::compiler_thread();
                let mut out = Vec::new();
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= sites.len() {
                        break;
                    }
                    let (fi, lo, hi) = sites[k];
                    let src = files[fi].1;
                    out.push((k, expand_item(sess, files[fi].0, &src[lo as usize..hi as usize], lo, crate::lexer::line_col_at(&lines[fi], src.len(), lo), Some(cfg), edition, None)));
                }
                out
            }));
        }
        let mut all = Vec::new();
        for h in hs {
            if let Ok(v) = h.join() {
                all.push(v);
            }
        }
        all
    });
    PRE.with(|m| {
        let mut m = m.borrow_mut();
        for v in results {
            for (k, r) in v {
                let (fi, lo, hi) = sites[k];
                let r = match r {
                    Ok(ItemOut::Done(x)) => Ok(Some(x)),
                    Ok(ItemOut::Nothing) => Ok(None),
                    Err(e) => Err(e),
                };
                m.insert((files[fi].0.to_string(), lo, hi), r);
            }
        }
    });
    crate::macro_reg::add_pre_wall(t_wall.elapsed().as_nanos() as u64);
}

/// Items carrying a `derive(..)` attribute that names a derive Relax runs (is_derive): `f(item lo, item hi)`
/// with the byte range the parser hands to `derive_items` (first outer attribute through the
/// item's last token). Items whose extent the scan cannot be sure of are skipped.
fn derive_sites(src: &[u8], toks: &[Tok], mut f: impl FnMut(u32, u32)) {
    let text = |i: usize| &src[toks[i].lo as usize..toks[i].hi as usize];
    let n = toks.len();
    let mut i = 0;
    while i < n {
        // an outer attribute run: `#[..]` `#[..]` .. then the item
        if toks[i].kind == T::Pound && i + 1 < n && toks[i + 1].kind == T::OpenBracket && (i == 0 || toks[i - 1].kind != T::Pound) {
            let start = i;
            let mut j = i;
            let mut has = false;
            while j + 1 < n && toks[j].kind == T::Pound && toks[j + 1].kind == T::OpenBracket {
                let close = match match_close(toks, j + 1) {
                    Some(c) => c,
                    None => return,
                };
                let first = text(j + 2);
                if first == b"derive" || first == b"cfg_attr" {
                    for k in j + 2..close {
                        if toks[k].kind == T::Ident {
                            let w = text(k);
                            if std::str::from_utf8(w).map_or(false, is_derive) {
                                has = true;
                            }
                        }
                    }
                }
                j = close + 1;
            }
            if has {
                // item: vis? (struct|enum|union) .. ending at a depth-0 `;` or the close of a
                // depth-0 `{ }`
                let mut k = j;
                if k < n && toks[k].kind == T::KwPub {
                    k += 1;
                    if k < n && toks[k].kind == T::OpenParen {
                        k = match match_close(toks, k) {
                            Some(c) => c + 1,
                            None => return,
                        };
                    }
                }
                let is_adt = k < n && (matches!(toks[k].kind, T::KwStruct | T::KwEnum) || (toks[k].kind == T::Ident && text(k) == b"union"));
                if is_adt {
                    let mut depth = 0i32;
                    let mut m = k;
                    let mut end = None;
                    while m < n {
                        match toks[m].kind {
                            T::OpenParen | T::OpenBracket => depth += 1,
                            T::CloseParen | T::CloseBracket => depth -= 1,
                            T::OpenBrace if depth == 0 => {
                                end = match_close(toks, m);
                                break;
                            }
                            T::OpenBrace => depth += 1,
                            T::CloseBrace => depth -= 1,
                            T::Semi if depth == 0 => {
                                end = Some(m);
                                break;
                            }
                            T::Eof => break,
                            _ => {}
                        }
                        m += 1;
                    }
                    if let Some(e) = end {
                        f(toks[start].lo, toks[e].hi);
                        i = e + 1;
                        continue;
                    }
                }
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
}

fn match_close(toks: &[Tok], open: usize) -> Option<usize> {
    let mut d = 0i32;
    for k in open..toks.len() {
        match toks[k].kind {
            T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
            T::CloseParen | T::CloseBracket | T::CloseBrace => {
                d -= 1;
                if d == 0 {
                    return Some(k);
                }
            }
            T::Eof => return None,
            _ => {}
        }
    }
    None
}

/// A derive Relax can run (through the JIT host).
fn is_derive(name: &str) -> bool {
    crate::macro_reg::has_derive(name)
}

/// Do the item's own leading `#[cfg(..)]` attributes hold?
fn item_cfg_on(trees: &[TokenTree], cfg: Option<&CfgSet>) -> bool {
    let mut i = 0;
    while i + 1 < trees.len() {
        match (&trees[i], &trees[i + 1]) {
            (TokenTree::Punct(p), TokenTree::Group(g)) if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket => {
                let inner = g.stream().trees;
                if let (Some(TokenTree::Ident(id)), Some(TokenTree::Group(args))) = (inner.first(), inner.get(1)) {
                    if id.to_string() == "cfg" && !cfg_holds(&args.stream().trees, cfg) {
                        return false;
                    }
                }
                i += 2;
            }
            _ => break,
        }
    }
    true
}

/// rustc's derive input: `cfg_attr` expanded, `#[cfg]`-false fields/variants removed (cfg_eval),
/// the `#[derive]` attributes taken off. Returns the native derive names in order and the item.
fn prepare_derive_input(input: TokenStream, cfg: Option<&CfgSet>) -> (Vec<String>, TokenStream) {
    let mut names = Vec::new();
    let trees = expand_cfg_attrs(input.trees, cfg);
    // take `#[derive(..)]` attributes off the item (leading attributes only)
    let mut out = Vec::new();
    let mut i = 0;
    while i < trees.len() {
        if let (TokenTree::Punct(p), Some(TokenTree::Group(g))) = (&trees[i], trees.get(i + 1)) {
            if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket {
                let inner = &g.stream().trees;
                if let Some(TokenTree::Ident(id)) = inner.first() {
                    if id.to_string() == "derive" {
                        if let Some(TokenTree::Group(args)) = inner.get(1) {
                            // last path segment of each derive
                            let mut last = String::new();
                            for t in &args.stream().trees {
                                match t {
                                    TokenTree::Ident(x) => last = x.to_string(),
                                    TokenTree::Punct(p) if p.as_char() == ',' => {
                                        if is_derive(&last) {
                                            names.push(std::mem::take(&mut last));
                                        }
                                        last.clear();
                                    }
                                    _ => {}
                                }
                            }
                            if is_derive(&last) {
                                names.push(last);
                            }
                        }
                        i += 2;
                        continue;
                    }
                }
            }
        }
        out.push(trees[i].clone());
        i += 1;
    }
    // cfg-strip fields / variants: the item's body groups
    let out = strip_body(out, cfg);
    (names, TokenStream { trees: out })
}

/// `#[cfg_attr(pred, a, b)]` -> `#[a] #[b]` when pred holds, nothing otherwise (outer
/// attributes at this level).
fn expand_cfg_attrs(trees: Vec<TokenTree>, cfg: Option<&CfgSet>) -> Vec<TokenTree> {
    let mut out = Vec::with_capacity(trees.len());
    let mut i = 0;
    while i < trees.len() {
        if let (TokenTree::Punct(p), Some(TokenTree::Group(g))) = (&trees[i], trees.get(i + 1)) {
            if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket {
                let inner = g.stream().trees;
                if let (Some(TokenTree::Ident(id)), Some(TokenTree::Group(args))) = (inner.first(), inner.get(1)) {
                    if id.to_string() == "cfg_attr" {
                        let parts = split_trees(&args.stream().trees);
                        if let Some((pred, rest)) = parts.split_first() {
                            if cfg_holds(pred, cfg) {
                                for a in rest {
                                    if a.is_empty() {
                                        continue;
                                    }
                                    let mut hash = Punct::new('#', Spacing::Alone);
                                    hash.set_span(p.span());
                                    out.push(TokenTree::Punct(hash));
                                    let mut ng = Group::new(Delimiter::Bracket, TokenStream { trees: a.clone() });
                                    ng.set_span(g.span());
                                    // nested cfg_attr
                                    let exp = expand_cfg_attrs(vec![out.pop().unwrap(), TokenTree::Group(ng)], cfg);
                                    out.extend(exp);
                                }
                            }
                        }
                        i += 2;
                        continue;
                    }
                }
            }
        }
        out.push(trees[i].clone());
        i += 1;
    }
    out
}

fn split_trees(trees: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    let mut v = vec![Vec::new()];
    for t in trees {
        if let TokenTree::Punct(p) = t {
            if p.as_char() == ',' {
                v.push(Vec::new());
                continue;
            }
        }
        v.last_mut().unwrap().push(t.clone());
    }
    if v.last().map_or(false, |x| x.is_empty()) {
        v.pop();
    }
    v
}

/// A cfg predicate over trees (`all(..)`, `any(..)`, `not(..)`, `name`, `name = "v"`).
pub fn cfg_holds(pred: &[TokenTree], cfg: Option<&CfgSet>) -> bool {
    let cfg = match cfg {
        Some(c) => c,
        None => return true,
    };
    match pred {
        [TokenTree::Ident(f), TokenTree::Group(g)] if g.delimiter() == Delimiter::Parenthesis => {
            let parts = split_trees(&g.stream().trees);
            match f.to_string().as_str() {
                "all" => parts.iter().all(|x| cfg_holds(x, Some(cfg))),
                "any" => parts.iter().any(|x| cfg_holds(x, Some(cfg))),
                "not" => parts.len() == 1 && !cfg_holds(&parts[0], Some(cfg)),
                _ => false,
            }
        }
        [TokenTree::Ident(n)] => cfg.set.iter().any(|x| *x == n.to_string()),
        [TokenTree::Ident(n), TokenTree::Punct(e), TokenTree::Literal(v)] if e.as_char() == '=' => {
            let v = v.to_string();
            { let k = format!("{}={}", n, v.trim_matches('"')); cfg.set.iter().any(|x| *x == k) }
        }
        _ => false,
    }
}

/// Removes `#[cfg(false)]` elements from the item's body: struct fields, tuple fields, enum
/// variants and their fields; expands cfg_attr on them.
fn strip_body(trees: Vec<TokenTree>, cfg: Option<&CfgSet>) -> Vec<TokenTree> {
    let is_enum = trees.iter().any(|t| matches!(t, TokenTree::Ident(i) if i.to_string() == "enum"));
    let mut out = Vec::with_capacity(trees.len());
    let mut seen_kw = false;
    for t in trees {
        if let TokenTree::Ident(i) = &t {
            let s = i.to_string();
            if s == "struct" || s == "enum" || s == "union" {
                seen_kw = true;
            }
        }
        match t {
            TokenTree::Group(g) if seen_kw && matches!(g.delimiter(), Delimiter::Brace | Delimiter::Parenthesis) => {
                let elems = strip_elems(g.stream().trees, cfg, is_enum);
                let mut ng = Group::new(g.delimiter(), TokenStream { trees: elems });
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            t => out.push(t),
        }
    }
    out
}

fn strip_elems(trees: Vec<TokenTree>, cfg: Option<&CfgSet>, variants: bool) -> Vec<TokenTree> {
    let trees = expand_cfg_attrs(trees, cfg);
    // split at top-level commas (generic args `<..,..>` in field types: track angle depth)
    let mut elems: Vec<Vec<TokenTree>> = vec![Vec::new()];
    let mut angle = 0i32;
    for t in trees {
        if let TokenTree::Punct(p) = &t {
            match p.as_char() {
                '<' => angle += 1,
                '>' => {
                    // `->` in fn pointer types is not a closing angle
                    let arrow = matches!(elems.last().and_then(|e| e.last()), Some(TokenTree::Punct(q)) if q.as_char() == '-' && q.spacing() == Spacing::Joint);
                    if !arrow {
                        angle -= 1;
                    }
                }
                ',' if angle <= 0 => {
                    elems.last_mut().unwrap().push(t);
                    elems.push(Vec::new());
                    continue;
                }
                _ => {}
            }
        }
        elems.last_mut().unwrap().push(t);
    }
    let mut out = Vec::new();
    for mut e in elems {
        // leading attributes: any `#[cfg(p)]` with p false drops the element
        let mut keep = true;
        let mut i = 0;
        while i + 1 < e.len() {
            match (&e[i], &e[i + 1]) {
                (TokenTree::Punct(p), TokenTree::Group(g)) if p.as_char() == '#' && g.delimiter() == Delimiter::Bracket => {
                    let inner = g.stream().trees;
                    if let (Some(TokenTree::Ident(id)), Some(TokenTree::Group(args))) = (inner.first(), inner.get(1)) {
                        if id.to_string() == "cfg" && !cfg_holds(&args.stream().trees, cfg) {
                            keep = false;
                        }
                    }
                    i += 2;
                }
                _ => break,
            }
        }
        if !keep {
            continue;
        }
        if variants {
            // a variant's own fields
            for t in e.iter_mut() {
                if let TokenTree::Group(g) = t {
                    if matches!(g.delimiter(), Delimiter::Brace | Delimiter::Parenthesis) {
                        let inner = strip_elems(g.stream().trees, cfg, false);
                        let mut ng = Group::new(g.delimiter(), TokenStream { trees: inner });
                        ng.set_span(g.span());
                        *t = TokenTree::Group(ng);
                    }
                }
            }
        }
        out.extend(e);
    }
    out
}

// ---------------------------------------------------------------- prescan

/// A function-like proc macro run by the JIT host (not built in): called at item
/// position, it may define `macro_rules!` (generic: nothing tells which ones do), so the crate's
/// macro prescan runs it and keeps what it defines -- as rustc, where a proc macro's output
/// macro_rules are visible to the items after it.
pub fn prescan_hosted(name: &str) -> bool {
    lookup(name) == Builtin::None && crate::macro_reg::bang_index(name).is_some()
}

/// Macro definitions a hosted item macro creates (makepad's `script_err_gen!(name)` defines
/// `macro_rules! name`): run at the crate's macro prescan so later files see them.
pub fn prescan_defs(cfg: &CfgSet, name: &str, arg_src: &[u8], krate: &str) -> Vec<crate::macros::MacroDef> {
    let sess = &cfg.session;
    let v = Vec::new();
    if !prescan_hosted(name) {
        return v;
    }
    let mut toks = Vec::new();
    if crate::lexer::lex(arg_src, 2021, &mut toks).is_err() {
        return v;
    }
    let span_of = |a: u32, z: u32| Span { file: 0, lo: a, hi: z, start: LineColumn { line: 1, column: 1 }, end: LineColumn { line: 1, column: 1 } };
    let input = match to_stream(arg_src, &toks, 0, toks.len() - 1, &span_of) {
        Some(s) => s,
        None => return v,
    };
    let r = match crate::macro_reg::bang_index(name) {
        Some(i) => crate::macro_reg::run_bang(sess, i, input, Span::DUMMY),
        None => return v,
    };
    let text = match r.map(|ts| render(&ts)) {
        Ok(t) => t,
        Err(_) => return v,
    };
    prescan_rules(text.as_bytes(), krate, cfg)
}

fn prescan_rules(tb: &[u8], krate: &str, cfg: &CfgSet) -> Vec<crate::macros::MacroDef> {
    let mut v = Vec::new();
    // `#[macro_export] macro_rules! name { .. }`
    let mut t2 = Vec::new();
    if crate::lexer::lex(tb, 2021, &mut t2).is_err() {
        return v;
    }
    let tx = |i: usize| &tb[t2[i].lo as usize..t2[i].hi as usize];
    let source = crate::cfg::Src { src: tb, plain: &t2, ext: &[], parts: None };
    let mut modules = vec![(0u32, crate::preload::file_cfg_active(tb, &t2, cfg))];
    let mut depth = 0u32;
    let mut i = 0;
    while i + 3 < t2.len() {
        match t2[i].kind {
            T::OpenBrace => depth += 1,
            T::CloseBrace => {
                if modules.len() > 1 && modules.last().unwrap().0 == depth { modules.pop(); }
                depth = depth.saturating_sub(1);
            }
            T::KwMod if t2[i + 2].kind == T::OpenBrace => {
                let attrs = crate::cfg::attrs_before(source, i, cfg);
                let active = modules.last().unwrap().1 && crate::cfg::active(source, &attrs, cfg)
                    && crate::preload::file_cfg_active(tb, &t2[i + 3..], cfg);
                modules.push((depth + 1, active));
            }
            _ => {}
        }
        if t2[i].kind == T::Ident && tx(i) == b"macro_rules" && t2[i + 1].kind == T::Not {
            let mname = String::from_utf8_lossy(tx(i + 2)).into_owned();
            let open = i + 3;
            let mut d = 0;
            let mut close = open;
            while close < t2.len() {
                match t2[close].kind {
                    T::OpenParen | T::OpenBracket | T::OpenBrace => d += 1,
                    T::CloseParen | T::CloseBracket | T::CloseBrace => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                close += 1;
            }
            let attrs = crate::cfg::attrs_before(source, i, cfg);
            if !modules.last().unwrap().1 || !crate::cfg::active(source, &attrs, cfg) {
                i = close + 1;
                continue;
            }
            let exported = attrs.iter().find(|a| a.toks.hi > a.toks.lo && tx(a.toks.lo as usize) == b"macro_export");
            let mt: Vec<crate::macros::MTok> =
                t2[open + 1..close.min(t2.len())].iter().map(|t| crate::macros::MTok { kind: t.kind, text: String::from_utf8_lossy(&tb[t.lo as usize..t.hi as usize]).into_owned(), pos: u32::MAX }).collect();
            if let Ok(mut def) = crate::macros::parse_def(&mname, krate, &mt) {
                def.exported = exported.is_some();
                if exported.is_some_and(|a| (a.toks.lo..a.toks.hi).any(|k| tx(k as usize) == b"local_inner_macros")) {
                    crate::macros::local_inner_macros(&mut def);
                }
                v.push(def);
            }
            i = close;
        }
        i += 1;
    }
    v
}

#[cfg(test)]
mod pm_input_tests {
    use super::*;

    #[test]
    fn prescan_cfg_definitions() {
        let source = br#"
            #[cfg(feature = "selected")]
            #[cfg_attr(all(), macro_export)]
            macro_rules! chosen { () => { 1 }; }
            #[cfg(not(feature = "selected"))]
            #[macro_export]
            macro_rules! chosen { () => { 2 }; }
            #[cfg(any())]
            mod inactive {
                #[macro_export]
                macro_rules! hidden { () => { 3 }; }
            }
            mod inactive_inner {
                #![cfg(any())]
                #[macro_export]
                macro_rules! hidden_inner { () => { 4 }; }
            }
            macro_rules! private { () => { 5 }; }
            #[cfg_attr(all(), cfg_attr(any(), cfg(any())))]
            macro_rules! retained { () => { 6 }; }
        "#;
        let session = std::sync::Arc::new(crate::Session::default());
        for features in [vec![], vec!["selected".to_string()]] {
            let cfg = CfgSet::host(&session, &features);
            let defs = prescan_rules(source, "fixture", &cfg);
            assert_eq!(defs.iter().map(|d| (d.name.as_str(), d.exported)).collect::<Vec<_>>(),
                [("chosen", true), ("private", false), ("retained", false)]);
        }
    }

    #[test]
    fn emitted_inner_attribute_keeps_pound_and_bang() {
        let source = b"#![allow(unused)] pub fn value() {}";
        let mut tokens = Vec::new();
        crate::lexer::lex(source, 2021, &mut tokens).unwrap_or_else(|e| panic!("{}", e.msg));
        let stream = crate::pm_tokens::from_toks(source, &tokens, &|lo, hi| Span { lo, hi, ..Span::DUMMY }).unwrap();
        let expansion = Expansion::from_stream(&stream, 2021).unwrap();
        assert_eq!(expansion.toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
            tokens.iter().filter(|t| t.kind != T::Eof).map(|t| t.kind).collect::<Vec<_>>());
    }

    #[test]
    fn emitted_origins_cover_raw_names_groups_literals_lifetimes_and_punctuation() {
        let span = Span { file: 7, lo: 40, hi: 90, ..Span::DUMMY };
        let punct = |ch, spacing| TokenTree::Punct(Punct { ch, spacing, span });
        let stream = TokenStream { trees: vec![TokenTree::Group(Group {
            delim: Delimiter::Brace, span,
            stream: TokenStream { trees: vec![
                TokenTree::Ident(Ident::new_raw("type", span)),
                TokenTree::Literal(Literal::with_span("\"x\"".into(), span)),
                punct('\'', Spacing::Joint), TokenTree::Ident(Ident::new("life", span)),
                punct('>', Spacing::Joint), punct('>', Spacing::Joint), punct('=', Spacing::Alone),
                punct(',', Spacing::Alone),
            ] },
        })] };
        let expansion = Expansion::from_stream(&stream, 2021).unwrap();
        assert_eq!(expansion.toks.iter().map(|t| t.kind).collect::<Vec<_>>(),
            [T::OpenBrace, T::RawIdent, T::Str, T::Lifetime, T::ShrEq, T::Comma, T::CloseBrace]);
        assert_eq!(expansion.origins.len(), expansion.toks.len());
        for (origin, token) in expansion.origins.iter().zip(&expansion.toks) {
            assert_eq!((origin.lo, origin.hi), (token.lo, token.hi));
            assert_eq!(origin.source, span.into());
            assert!(origin.hi - origin.lo < span.hi - span.lo);
        }
        assert_eq!(&expansion.text[expansion.origins[1].lo as usize..expansion.origins[1].hi as usize], "r#type");
        // A lexer token joined from punctuation authored in two files keeps both origins.
        let mut other = span; other.file = 8;
        let stream = TokenStream { trees: vec![punct(':', Spacing::Joint), TokenTree::Punct(Punct { ch: ':', spacing: Spacing::Alone, span: other })] };
        let expansion = Expansion::from_stream(&stream, 2021).unwrap();
        assert_eq!(expansion.toks[0].kind, T::PathSep);
        assert_eq!(crate::source::origin_at(&expansion.origins, 0), Some(span.into()));
        assert_eq!(crate::source::origin_at(&expansion.origins, 1), Some(other.into()));
    }

    #[test]
    fn nested_macro_input_keeps_full_imported_span() {
        let sess = std::sync::Arc::new(crate::Session::default());
        let sources = std::sync::Arc::new(crate::source::SourceRegistry::default());
        let src = b"authored module";
        sources.register("/fake/module.rs", std::sync::Arc::from(&src[..])).unwrap();
        let text = "// imported\néé long_authored_construct\n";
        let file = sources.register("/fake/import.rs", std::sync::Arc::from(text.as_bytes())).unwrap();
        let source = SourceSpan { file, lo: text.find("long").unwrap() as u32, hi: text.trim_end().len() as u32 };
        let input = TokenStream { trees: vec![TokenTree::Ident(Ident::new_raw("type", sources.get(file).unwrap().token_span(source)))] };
        let output = Expansion::from_stream(&input, 2021).unwrap();
        let base = src.len() as u32;
        let mut p = crate::parser::Parser::new(&sess, src, output.toks.iter().map(|t| Tok { kind: t.kind, lo: base + t.lo, hi: base + t.hi }).collect::<Vec<_>>(), 2021);
        p.ext = output.text.into_bytes().into();
        p.token_origins = output.origins.iter().map(|o| o.shifted(base)).collect::<Vec<_>>().into();
        p.token_input = true;
        p.sources = Some(sources);
        p.bctx = Some(std::rc::Rc::new(FileCtx::new("test", "/fake/module.rs", "test", &CfgSet::host(&sess, &[]))));
        let start = p.expand_text("forward", "r#type", base).unwrap_or_else(|e| panic!("{}", e.msg));
        let nested = p.toks.at(start).lo;
        p.tok_origin.push((nested, base, 6));
        for needed in [false, true] {
            expand_pm_ts(&p, needed, start, start + 1, nested, nested + 6, |tokens, call| {
                let span = tokens.trees[0].span();
                assert_eq!(SourceSpan::from(span), source);
                assert_eq!(span.start, LineColumn { line: 2, column: 4 });
                assert_eq!(span, call);
                Ok(tokens)
            }).unwrap();
        }
    }

    #[test]
    fn pm_indexed_columns_match_byte_scan() {
        // Include positions inside UTF-8 characters, a trailing newline, EOF and
        // past EOF: the old routine clamps bytes, it does not require char boundaries.
        for src in ["", "ascii\n", "é中🦀\n xé\r\n\nend", "é\u{301}x"] {
            let src = src.as_bytes();
            let ls = line_starts(src);
            let cs = continuation_bytes(src);
            let cursor = std::cell::Cell::new(ColumnCursor::default());
            let end = src.len() as u32 + 3;
            for pos in (0..end).chain((0..end).rev()).chain([end, 0, end / 2, 1, end]) {
                assert_eq!(indexed_line_col(src, &ls, &cs, &cursor, pos), line_col(src, &ls, pos));
            }
        }
    }

    fn compare(p: &crate::parser::Parser, lo: usize, hi: usize, call_lo: u32, call_hi: u32) {
        let (expected, expected_call) = legacy_pm_input(p, true, lo, hi, call_lo, call_hi, false);
        expand_pm_ts(p, true, lo, hi, call_lo, call_hi, |actual, call| {
            verify_pm_spans(&actual, &expected);
            assert_eq!(call, expected_call);
            Ok(actual)
        }).unwrap();
    }

    #[test]
    fn pm_input_source_fragments_and_unmapped_expansions() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let src = "α ('life, [r#name, \"é中🦀\", a >>= 2])\n/// doc é\nlast".as_bytes();
        let mut toks = Vec::new();
        crate::lexer::lex(src, 2021, &mut toks).unwrap_or_else(|e| panic!("{}", e.msg));
        let mut p = crate::parser::Parser::new(sess, src, toks, 2021);
        p.bctx = Some(std::rc::Rc::new(FileCtx::new("test", "/tmp/pm-input.rs", "test", &CfgSet::host(sess, &[]))));
        compare(&p, 0, p.toks.len(), 0, src.len() as u32);
        // An unrelated large expansion must not affect source spans or require an
        // expansion index. Deliberately use a single long line, as generated text does.
        p.ext.extend(std::iter::repeat_n(b' ', 2_000_000 - p.ext.len()));
        compare(&p, 0, p.toks.len(), 0, src.len() as u32);
        let start = p.expand_text("test", "(forwarded >>= 2)", 0).unwrap_or_else(|e| panic!("{}", e.msg));
        let base = p.toks.at(start).lo;
        p.tok_origin.push((base + 1, 0, 2));
        compare(&p, start, p.toks.len(), base, p.toks.last().unwrap().hi);
        // Nested substitution, resolving a fragment through the preceding expansion.
        let start = p.expand_text("test", "[forwarded]", base).unwrap_or_else(|e| panic!("{}", e.msg));
        let nested = p.toks.at(start).lo;
        p.tok_origin.push((nested + 1, base + 1, 2));
        compare(&p, start, p.toks.len(), nested, p.toks.last().unwrap().hi);
        // No call-site mapping: exercise the lazy expansion index, Unicode and EOF.
        p.exp_map.clear();
        p.tok_origin.clear();
        let start = p.expand_text("test", "\n(\"é中🦀\", x)", u32::MAX).unwrap_or_else(|e| panic!("{}", e.msg));
        p.exp_map.clear();
        compare(&p, start, p.toks.len(), p.toks.at(start).lo, p.toks.last().unwrap().hi);
    }
}

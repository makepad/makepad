//! Interface dependencies by item (JITCOLD2): what a code key names of the program's interfaces.
//!
//! A crate's interface fingerprint (items.rs crate_ifaces) changes with any item of the crate or
//! of anything below it: keying code by it recompiled the whole app after any edit outside a fn
//! body (a new fn, a field, a `use`), and everything above a framework crate after any edit of
//! that crate's interface. Here each item of a non-sysroot crate is a node of one graph:
//!   - its piece: the item's text with its attributes, fn bodies and static initializers cut out
//!     (the holes of items.rs); a container (a module, an inherent impl, an extern block) only its
//!     header, its items are nodes of their own; a trait and a trait impl one node with their items;
//!   - its names: every identifier in the piece (strings and comments included: an inline format
//!     argument names a const), except the item's own defining name;
//!   - the names it is found under (keys): an item's name (an enum also its variants', a trait
//!     also its items'), a `use` its binding names, an inherent impl's fn/const/type its own name,
//!     a trait impl the names of its self type (and its trait's name when the self type is a
//!     generic parameter, a primitive, an alias or no path: such an impl can apply to types
//!     named nowhere in the program's text).
//! A node depends on every node found under one of its names in the crates its crate can see
//! (its universe: itself and its dependencies), and an inherent impl's item on its impl's header.
//! A node's closure hash covers the pieces of everything it reaches (one Merkle hash per
//! strongly connected component). Some nodes are reached by every node of the crates that can see
//! them (`always`): the files' inner attributes, glob and `as _` imports (they bring unnamed
//! names and traits into scope) and item macros left unexpanded.
//!
//! Soundness: everything the code of a fn can take from another item reaches it through a name in
//! some text the fn's key covers, or through a trait impl found by its self type:
//!   - name resolution (paths, methods, fields, macros) can only find an item under one of the
//!     names written; a new item with a written name changes the set found under it;
//!   - a type's layout, a signature, a const's value, a trait's items come from the pieces of the
//!     items named, and their own names reach the items they are made of (transitively);
//!   - trait selection for a type finds impls whose self type names that type (indexed under
//!     those names), or impls that apply to unnamed types (indexed under the trait's name, which
//!     the bound or call that asks for the trait names).
//! What a name cannot show is kept coarse: the sysroot crates (core, alloc, std: lang items and the
//! expansions of built-in macros name them implicitly) are not in the graph, every key names their
//! whole interface fingerprints (`atomic`); a piece invoking a macro outside the item position
//! (a const initialized by a proc macro: its expansion may name anything) depends on its crate's
//! whole interface fingerprint (`coarse`). A fn body invoking such macros gets the names of its
//! expansions (`exp_names`, recorded with its code: cache.rs KIND_EXP_NAMES).
use crate::ast::{ItemKind, UseTree};
use crate::parser::ParsedFile;
use crate::program::{DefId, DefKind, Program, NO_DEF};
use crate::sip::Sip128;
use std::collections::HashMap;

/// An identifier, hashed (two names with one hash are one name: more dependencies, never fewer).
pub(crate) type Name = u64;

pub(crate) fn name_of(b: &[u8]) -> Name {
    // FNV-1a over the bytes, finished with a mix (names are short)
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^ (h >> 33)
}

fn is_ident_start(c: u8) -> bool { c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 }
fn is_ident_char(c: u8) -> bool { c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 }

/// Keywords never name an item (skipped: they would only connect unrelated pieces).
fn keyword(w: &[u8]) -> bool {
    matches!(w, b"as" | b"async" | b"await" | b"break" | b"const" | b"continue" | b"crate" | b"dyn" | b"else" | b"enum"
        | b"extern" | b"false" | b"fn" | b"for" | b"if" | b"impl" | b"in" | b"let" | b"loop" | b"match" | b"mod"
        | b"move" | b"mut" | b"pub" | b"ref" | b"return" | b"self" | b"Self" | b"static" | b"struct" | b"super"
        | b"trait" | b"true" | b"type" | b"unsafe" | b"use" | b"where" | b"while" | b"r" | b"_")
}

/// Macros whose expansion names nothing the invocation does not (built-ins producing literals or
/// sysroot paths, and sysroot macros of that kind; `write!` names `write_fmt`, which `scan` adds)
/// -- while no non-sysroot macro of the name exists (`Deps::macros`).
fn quiet_macro(w: &[u8]) -> bool {
    // std's `is_x86_feature_detected!` and friends: a sysroot check
    (w.starts_with(b"is_") && w.ends_with(b"_feature_detected")) || matches!(w, b"macro_rules" | b"concat" | b"stringify" | b"line" | b"column" | b"file" | b"module_path" | b"env"
        | b"option_env" | b"include_str" | b"include_bytes" | b"cfg" | b"compile_error" | b"matches" | b"assert"
        | b"assert_eq" | b"assert_ne" | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne" | b"panic"
        | b"unreachable" | b"todo" | b"unimplemented" | b"format_args" | b"format" | b"vec" | b"print" | b"println"
        | b"eprint" | b"eprintln" | b"dbg" | b"offset_of" | b"addr_of" | b"addr_of_mut" | b"pin" | b"ready"
        | b"assert_matches" | b"debug_assert_matches" | b"format_args_nl" | b"const_format_args" | b"write" | b"writeln")
}

/// The identifiers of `src[a..b]` for each segment, minus the spans in `skip`, into `names`; the
/// macros invoked there (`name!(`, `name![`, `name!{`) into `invoked`. Comments (doc comments
/// too) name nothing; a string literal names what a format string's inline arguments name
/// (`{name}`, `{:width$}`), nothing else.
pub(crate) fn scan(src: &[u8], segs: &[(u32, u32)], skip: &[(u32, u32)], names: &mut Vec<Name>, invoked: &mut Vec<Name>) {
    scan_at(src, segs, skip, names, invoked, &mut Vec::new())
}

/// `scan`, also giving each invocation's position and the end of its arguments (`at`: two entries
/// per `invoked` entry).
fn scan_at(src: &[u8], segs: &[(u32, u32)], skip: &[(u32, u32)], names: &mut Vec<Name>, invoked: &mut Vec<Name>, at: &mut Vec<u32>) {
    for &(a, b) in segs {
        let (a, b) = (a as usize, (b as usize).min(src.len()));
        let mut i = a;
        // inside a `macro_rules!` definition (to this position): its rules invoke nothing here (the
        // invocations of the macro are expanded where they are called)
        let mut rules_end = 0usize;
        while i < b {
            let c = src[i];
            match c {
                b'/' if src.get(i + 1) == Some(&b'/') => {
                    while i < b && src[i] != b'\n' { i += 1; }
                    continue;
                }
                b'/' if src.get(i + 1) == Some(&b'*') => {
                    i = skip_block_comment(src, i, b);
                    continue;
                }
                b'"' => {
                    i = string(src, i + 1, b, 0, names);
                    continue;
                }
                b'\'' => {
                    i = char_or_lifetime(src, i, b);
                    continue;
                }
                _ => {}
            }
            if c.is_ascii_digit() {
                // a number with its suffix (`1u8`, `0x1f`) names nothing
                while i < b && is_ident_char(src[i]) { i += 1; }
                continue;
            }
            if !is_ident_start(c) { i += 1; continue; }
            let s = i;
            while i < b && is_ident_char(src[i]) { i += 1; }
            let w = &src[s..i];
            // a raw string (`r"..."`, `r#"..."#`, `br#"..."#`, `cr"..."`)
            if matches!(w, b"r" | b"br" | b"cr") && i < b && (src[i] == b'"' || src[i] == b'#') {
                let mut h = 0;
                let mut j = i;
                while j < b && src[j] == b'#' { h += 1; j += 1; }
                if j < b && src[j] == b'"' {
                    i = string(src, j + 1, b, h + 1, names);
                    continue;
                }
            }
            // a byte or C string (`b"..."`, `c"..."`)
            if matches!(w, b"b" | b"c") && i < b && src[i] == b'"' {
                i = string(src, i + 1, b, 0, names);
                continue;
            }
            if skip.iter().any(|&(x, y)| s as u32 >= x && (s as u32) < y) { continue; }
            if keyword(w) { continue; }
            let n = name_of(w);
            names.push(n);
            // an invocation: `!` (not `!=`), then an opening bracket, whitespace allowed between
            let mut j = i;
            while j < b && src[j].is_ascii_whitespace() { j += 1; }
            if j < b && src[j] == b'!' && src.get(j + 1) != Some(&b'=') {
                let mut k = j + 1;
                while k < b && src[k].is_ascii_whitespace() { k += 1; }
                if k < b && matches!(src[k], b'(' | b'[' | b'{') && s >= rules_end {
                    invoked.push(if quiet_macro(w) { n | QUIET_BIT } else { n & !QUIET_BIT });
                    at.push(s as u32);
                    at.push(group_end(src, k, b) as u32);
                    // the parser lowers std's `write!`/`writeln!` itself (no expansion text): to a
                    // `write_fmt` method call on the destination
                    if matches!(w, b"write" | b"writeln") { names.push(name_of(b"write_fmt")); }
                }
                // `macro_rules! name { .. }`: its body is rules, not invocations
                if w == b"macro_rules" && s >= rules_end {
                    let mut k = j + 1;
                    while k < b && (src[k].is_ascii_whitespace() || is_ident_char(src[k])) { k += 1; }
                    if k < b && matches!(src[k], b'(' | b'[' | b'{') { rules_end = group_end(src, k, b); }
                }
            }
        }
    }
}

/// Past the bracket group opening at `i` (any bracket kind counted; strings, chars and comments
/// skipped).
fn group_end(src: &[u8], mut i: usize, b: usize) -> usize {
    let mut depth = 0i32;
    while i < b {
        match src[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => { depth -= 1; if depth == 0 { return i + 1; } }
            b'"' => { i = string(src, i + 1, b, 0, &mut Vec::new()); continue; }
            b'\'' => { i = char_or_lifetime(src, i, b); continue; }
            b'/' if src.get(i + 1) == Some(&b'/') => { while i < b && src[i] != b'\n' { i += 1; } continue; }
            b'/' if src.get(i + 1) == Some(&b'*') => { i = skip_block_comment(src, i, b); continue; }
            _ => {}
        }
        i += 1;
    }
    b
}

/// Past a block comment starting at `i` (nested ones included).
fn skip_block_comment(src: &[u8], mut i: usize, b: usize) -> usize {
    let mut depth = 0u32;
    while i < b {
        if src[i] == b'/' && src.get(i + 1) == Some(&b'*') { depth += 1; i += 2; continue; }
        if src[i] == b'*' && src.get(i + 1) == Some(&b'/') {
            i += 2;
            depth -= 1;
            if depth == 0 { return i; }
            continue;
        }
        i += 1;
    }
    b
}

/// Past a string literal whose text starts at `i` (`hashes` 0: an ordinary string with escapes; n:
/// a raw string closed by `"` and n - 1 `#`), its format inline arguments into `names`.
fn string(src: &[u8], mut i: usize, b: usize, hashes: usize, names: &mut Vec<Name>) -> usize {
    while i < b {
        let c = src[i];
        if hashes == 0 && c == b'\\' { i += 2; continue; }
        if c == b'"' {
            let close = hashes.saturating_sub(1);
            if (0..close).all(|k| src.get(i + 1 + k) == Some(&b'#')) { return i + 1 + close; }
        }
        // `{name` (not `{{`), and `name$` (a width or precision argument)
        if c == b'{' && src.get(i + 1) == Some(&b'{') { i += 2; continue; }
        if c == b'{' || (is_ident_start(c) && i > 0 && !is_ident_char(src[i - 1])) {
            let s = if c == b'{' { i + 1 } else { i };
            let mut j = s;
            while j < b && is_ident_char(src[j]) { j += 1; }
            if j > s && is_ident_start(src[s]) && (c == b'{' || src.get(j) == Some(&b'$')) {
                let w = &src[s..j];
                if !keyword(w) { names.push(name_of(w)); }
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }
    b
}

/// Past a char literal or a lifetime/label at `i` (a `'`).
fn char_or_lifetime(src: &[u8], i: usize, b: usize) -> usize {
    // an escape: `'\n'`, `'\''`, `'\u{1F600}'`
    if src.get(i + 1) == Some(&b'\\') {
        let mut j = i + 3;
        while j < b && j < i + 14 && src[j] != b'\'' { j += 1; }
        return (j + 1).min(b);
    }
    // one char (one UTF-8 sequence) and a closing quote
    if let Some(&c) = src.get(i + 1) {
        let len = if c < 0x80 { 1 } else if c >= 0xf0 { 4 } else if c >= 0xe0 { 3 } else { 2 };
        if src.get(i + 1 + len) == Some(&b'\'') { return i + 2 + len; }
    }
    // a lifetime or label: its name is scanned as an identifier (it names nothing that matters)
    i + 1
}

/// `scan`'s marker on an invocation of a quiet macro (its name with the bit set): quiet unless a
/// user macro of that name exists.
pub(crate) const QUIET_BIT: Name = 1 << 63;

/// One node, as a file's walk makes it.
struct Raw {
    /// the items it stands for (the first is its own)
    items: Vec<u32>,
    piece: u128,
    names: Vec<Name>,
    keys: Vec<Name>,
    /// loud macros invoked in its text whose expansion it does not have (deps.rs file_nodes)
    invoked: Vec<Name>,
    /// where (diagnostics: MAKEPAD_RUN_DEPS_DUMP)
    invoked_at: Vec<u32>,
    always: bool,
    /// a macro item left unexpanded: coarse whatever it invokes
    mac: bool,
    /// a macro definition: its text is not expanded here
    rules: bool,
    /// the item of the header it belongs to (an inherent impl's or extern block's item)
    header: Option<u32>,
    /// a trait impl: its trait's name, and whether its self type's head is a named path (else it
    /// is also found under the trait's name; so is one whose self type names an alias)
    trait_name: Option<Name>,
    named_head: bool,
}

/// One node's closure, by its item (Deps::nodes, sorted by (file, item)).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NodeRow {
    file: u32,
    item: u32,
    closure: [u64; 2],
}

/// What a name finds in one crate (Deps::names, sorted by (name, crate)), and the strongly connected
/// component its name node is in (an id unique within the graph: a graph built over a stored one
/// keeps the ids of the rows it takes over).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NameRow {
    name: u64,
    krate: u32,
    comp: u32,
    closure: [u64; 2],
}

/// One crate as the graph was built from it (Deps::crates, by crate index): what its rows depend on
/// besides the crates it can see (whose rows say the same of them), and where its own lists are.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct CrateRow {
    /// its name and root file (crate_ident)
    ident: [u64; 2],
    /// its graph files: their numbers, paths and item counts (the rows name items by file and item)
    files: [u64; 2],
    /// its interface fingerprint: its files' text outside the holes (what its nodes are made of:
    /// pieces and their names; also what its coarse nodes are keyed by). Not its fingerprint: an
    /// edit inside fn bodies keeps every row of its crate (a body is in no piece)
    iface: [u64; 2],
    /// the closures of the always nodes of every crate it can see (Deps::always)
    always: [u64; 2],
    /// its own always nodes' closures, alias names and macro names: ranges of Deps::lists
    always_at: [u32; 2],
    aliases_at: [u32; 2],
    macros_at: [u32; 2],
    atomic: u32,
    _pad: u32,
}

fn h2(x: u128) -> [u64; 2] { [x as u64, (x >> 64) as u64] }
fn u2(x: [u64; 2]) -> u128 { x[0] as u128 | (x[1] as u128) << 64 }

/// A table built here or mapped from a store entry (`Deps::store` keeps the mapping).
enum Rows<T: 'static> {
    Owned(Vec<T>),
    Mapped(&'static [T]),
}
impl<T> Rows<T> {
    fn rows(&self) -> &[T] {
        match self { Rows::Owned(v) => v, Rows::Mapped(s) => s }
    }
}

/// The program's item graph, as what a key asks of it: each item node's closure hash and what
/// each name finds per crate.
pub(crate) struct Deps {
    /// what it was built from (Deps::valid)
    valid: u128,
    nodes: Rows<NodeRow>,
    names: Rows<NameRow>,
    /// names of non-sysroot macros (macro_rules, proc macros), sorted: a quiet macro of such a
    /// name is not quiet
    macros: Rows<u64>,
    crates: Rows<CrateRow>,
    /// the crates' own lists (CrateRow ranges): always closures as two words each, names as one
    lists: Rows<u64>,
    /// the component ids in use are below this
    comp_end: u32,
    /// per crate: the crates it can see (itself and its dependencies, transitively), as bits
    universe: Vec<Vec<u64>>,
    /// per crate: not in the graph (sysroot), keyed by its interface fingerprint
    pub(crate) atomic: Vec<bool>,
    /// the store a mapped graph's tables are in
    _store: Option<std::sync::Arc<crate::store::Store>>,
    pub(crate) nodes_n: usize,
    /// the nodes this build walked (the others' rows came from the stored graph it was built over)
    pub(crate) walked: usize,
    pub(crate) edges: usize,
    pub(crate) coarse: usize,
    /// build phase ends (ms): files walked, edges, components, hashed (a mapped graph: zeros)
    pub(crate) phases: [f64; 4],
}

const DEPS_MAGIC: &[u8; 4] = b"DEP2";
const DEPS_VERSION: u32 = 3;

fn bit(set: &[u64], c: u32) -> bool { set.get(c as usize / 64).is_some_and(|w| w >> (c % 64) & 1 == 1) }

/// The crates each crate can see: itself and its dependencies, transitively.
fn universes(prog: &Program) -> Vec<Vec<u64>> {
    let n = prog.crates.len();
    let words = n.div_ceil(64).max(1);
    let mut out: Vec<Vec<u64>> = vec![Vec::new(); n];
    let mut done = vec![false; n];
    fn go(c: usize, prog: &Program, out: &mut Vec<Vec<u64>>, done: &mut Vec<bool>, words: usize, depth: u32) {
        if done[c] { return; }
        let mut v = vec![0u64; words];
        v[c / 64] |= 1 << (c % 64);
        // (a cycle cannot occur in a crate graph; the depth bound only guards a broken one)
        if depth < 1024 {
            for &(_, d) in &prog.crates[c].deps {
                let d = d as usize;
                if d == c || d >= out.len() { continue; }
                go(d, prog, out, done, words, depth + 1);
                for (a, b) in v.iter_mut().zip(&out[d]) { *a |= *b; }
            }
        }
        out[c] = v;
        done[c] = true;
    }
    for c in 0..n { go(c, prog, &mut out, &mut done, words, 0); }
    out
}

/// Whether crate `c` is a sysroot crate (core, alloc, std and what they are built from): rooted in
/// the std image's directory.
fn sysroot(prog: &Program, c: usize, std_dir: &str) -> bool {
    let ci = &prog.crates[c];
    let root = prog.mods.get(ci.root_mod as usize).and_then(|m| prog.file_paths.get(m.file as usize)).map_or("", |p| p.as_str());
    (!std_dir.is_empty() && root.starts_with(std_dir)) || matches!(prog.syms.str(ci.name), "core" | "alloc" | "std" | "proc_macro" | "test")
}

/// Per crate: not in the graph (keyed by its whole fingerprint in its view, items.rs crate_views):
/// the sysroot crates, proc-macro crates (their code makes expansions, never JIT code of the program)
/// and the crates loaded after the cache opened (`ncr` crates are the cache's).
pub(crate) fn atomic_crates(prog: &Program, ncr: usize) -> Vec<bool> {
    let ncr = ncr.min(prog.crates.len());
    let std_dir = crate::std_dir();
    (0..prog.crates.len()).map(|c| c >= ncr || sysroot(prog, c, &std_dir) || prog.crates[c].proc_macro).collect()
}

/// The items reached from a file's root (through modules, impls, traits, extern blocks), each with
/// the item that contains it.
fn reached(pf: &ParsedFile) -> Vec<(u32, Option<u32>)> {
    let ast = &pf.ast;
    let mut out = Vec::new();
    let mut seen = vec![false; ast.items.len()];
    let mut stack: Vec<(u32, Option<u32>)> = ast.root_items.iter().map(|i| (i.0, None)).collect();
    stack.reverse();
    while let Some((i, parent)) = stack.pop() {
        let Some(s) = seen.get_mut(i as usize) else { continue };
        if std::mem::replace(s, true) { continue; }
        out.push((i, parent));
        let kids: &[crate::ast::ItemId] = match &ast.item(crate::ast::ItemId(i)).kind {
            ItemKind::Trait(r) => ast.list(ast.row(*r).items),
            ItemKind::Impl(r) => ast.list(ast.row(*r).items),
            ItemKind::ForeignMod(r) => ast.list(ast.row(*r).items),
            ItemKind::Mod(Some(items)) => ast.list(*items),
            _ => &[],
        };
        for k in kids.iter().rev() { stack.push((k.0, Some(i))); }
    }
    out
}

/// The span of an item with its attributes.
fn full_span(pf: &ParsedFile, i: u32) -> (u32, u32) {
    let ast = &pf.ast;
    let it = ast.item(crate::ast::ItemId(i));
    let mut lo = it.lo;
    for a in ast.list(it.attrs) {
        if a.toks.hi > a.toks.lo { lo = lo.min(pf.tok(a.toks.lo).lo); }
    }
    (lo, it.hi)
}

fn attr_segs(pf: &ParsedFile, attrs: &[crate::ast::Attr], out: &mut Vec<(u32, u32)>) {
    for a in attrs {
        if a.toks.hi > a.toks.lo { out.push((pf.tok(a.toks.lo).lo, pf.tok(a.toks.hi - 1).hi)); }
    }
}

/// `segs` minus the spans `cut` (both by position).
fn minus(mut segs: Vec<(u32, u32)>, cut: &mut Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    segs.sort_unstable();
    cut.sort_unstable();
    let mut out = Vec::with_capacity(segs.len() + cut.len());
    for (a, b) in segs {
        let mut at = a;
        let k = cut.partition_point(|x| x.1 <= at);
        for &(ca, cb) in &cut[k..] {
            if ca >= b { break; }
            if ca > at { out.push((at, ca)); }
            at = at.max(cb);
            if at >= b { break; }
        }
        if at < b { out.push((at, b)); }
    }
    out
}

/// A piece's hash: its segments' text without comments, whitespace runs as one space (string
/// literals as written): an edit of a comment or of the layout of the text keeps it.
fn piece_hash(src: &[u8], kind: u32, segs: &[(u32, u32)]) -> u128 {
    let mut h = Sip128::new();
    h.str("dep-piece");
    h.u32(kind);
    let mut buf: Vec<u8> = Vec::new();
    for &(a, b) in segs {
        let (a, b) = ((a as usize).min(src.len()), (b as usize).min(src.len()));
        buf.clear();
        let mut i = a;
        let mut space = false;
        while i < b {
            let c = src[i];
            let skip_to = if c == b'/' && src.get(i + 1) == Some(&b'/') {
                let mut j = i;
                while j < b && src[j] != b'\n' { j += 1; }
                Some(j)
            } else if c == b'/' && src.get(i + 1) == Some(&b'*') {
                Some(skip_block_comment(src, i, b))
            } else {
                None
            };
            if let Some(j) = skip_to { space = true; i = j; continue; }
            if c.is_ascii_whitespace() { space = true; i += 1; continue; }
            if space && !buf.is_empty() { buf.push(b' '); }
            space = false;
            let end = match c {
                b'"' => string(src, i + 1, b, 0, &mut Vec::new()),
                b'\'' => char_or_lifetime(src, i, b),
                _ => {
                    // a raw string keeps its text: `r#"..."#` is copied whole
                    if (c == b'r' || c == b'b' || c == b'c') && (i == a || !is_ident_char(src[i - 1])) {
                        let mut j = i + 1;
                        if c != b'r' && src.get(j) == Some(&b'r') { j += 1; }
                        let mut hs = 0;
                        while j < b && src[j] == b'#' { hs += 1; j += 1; }
                        if j < b && src[j] == b'"' && (src[i + 1] == b'r' || c == b'r' || hs == 0) {
                            string(src, j + 1, b, if c == b'r' || src[i + 1] == b'r' { hs + 1 } else { 0 }, &mut Vec::new())
                        } else { i + 1 }
                    } else { i + 1 }
                }
            };
            let end = end.max(i + 1).min(b);
            buf.extend_from_slice(&src[i..end]);
            i = end;
        }
        h.write(&buf);
        h.u8(0xfe);
    }
    h.finish128()
}

/// The names in a type (path segments, generic arguments), and whether its head is a named path
/// (not a reference, tuple, slice, fn pointer, trait object or a single-segment path naming one of
/// `params` or a primitive).
fn ty_names(pf: &ParsedFile, t: crate::ast::TyId, params: &[&str], out: &mut Vec<Name>, depth: u32) -> bool {
    use crate::ast::Ty;
    if depth > 32 { return false; }
    let ast = &pf.ast;
    match ast.ty(t) {
        Ty::Path(p) => {
            let segs = ast.segs(p.segs);
            for s in segs {
                let w = pf.text(s.name);
                if !w.is_empty() && !keyword(w.as_bytes()) { out.push(name_of(w.as_bytes())); }
                if let Some(ga) = ast.gargs(s.args()) {
                    match ga {
                        crate::ast::GenericArgs::Angle(l) => for g in ast.list(*l) {
                            match g {
                                crate::ast::GenericArg::Type(t) => { ty_names(pf, *t, params, out, depth + 1); }
                                crate::ast::GenericArg::Binding(_, _, t) => { ty_names(pf, *t, params, out, depth + 1); }
                                _ => {}
                            }
                        },
                        crate::ast::GenericArgs::Paren(l, r) => {
                            for t in ast.list(*l) { ty_names(pf, *t, params, out, depth + 1); }
                            if let Some(r) = r { ty_names(pf, *r, params, out, depth + 1); }
                        }
                    }
                }
            }
            if ast.qself(p).is_some() { return false; }
            match segs {
                [one] => {
                    let w = pf.text(one.name);
                    !(params.contains(&w) || primitive(w) || w == "Self")
                }
                [] => false,
                _ => true,
            }
        }
        Ty::Paren(t) => ty_names(pf, *t, params, out, depth + 1),
        Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Array(t, _) => { ty_names(pf, *t, params, out, depth + 1); false }
        Ty::Tuple(l) => { for t in ast.list(*l) { ty_names(pf, *t, params, out, depth + 1); } false }
        _ => false,
    }
}

fn primitive(w: &str) -> bool {
    matches!(w, "bool" | "char" | "str" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "u128" | "usize" | "f16" | "f32" | "f64" | "f128")
}

/// The binding names of a use tree (`self` binds its prefix's last segment), and whether it has a
/// glob or an `as _`.
fn use_names(pf: &ParsedFile, t: &UseTree, last: Option<&str>, out: &mut Vec<Name>, glob: &mut bool, depth: u32) {
    if depth > 64 { *glob = true; return; }
    let ast = &pf.ast;
    match t {
        UseTree::Path(seg, sub) => {
            let w = pf.text(seg.name);
            use_names(pf, ast.use_tree(*sub), Some(w), out, glob, depth + 1);
        }
        UseTree::Name(seg, rename) => {
            let w = pf.text(seg.name);
            let bound = match rename {
                Some(r) => pf.text(*r),
                None if w == "self" => last.unwrap_or(w),
                None => w,
            };
            if bound == "_" { *glob = true; }
            out.push(name_of(bound.as_bytes()));
        }
        UseTree::Glob => *glob = true,
        UseTree::Group(l) => for t in ast.list(*l) { use_names(pf, t, last, out, glob, depth + 1) },
    }
}

/// The holes of a file (fn bodies, static initializers outside its interface: items.rs).
fn holes(dg: &super::items::FileDigest) -> Vec<(u32, u32)> {
    dg.hole_spans()
}

/// One file's nodes.
fn file_nodes(pf: &ParsedFile, dg: &super::items::FileDigest, path: &str) -> Vec<Raw> {
    let ast = &pf.ast;
    let src: &[u8] = &pf.src;
    let all_holes = holes(dg);
    let mut out: Vec<Raw> = Vec::new();
    // the file's macro expansions by direct call site (the file's text and its lazily parsed
    // bodies'; Deps::build parses the parts the graph needs first, so the graph never depends on
    // when it is built)
    let regions = file_regions(pf);
    // what the expansions called from within `segs` (and the expansions called from those) name;
    // and the invocations in `segs` that have none (their names are unknown: the node is coarse)
    let expanded = |segs: &[(u32, u32)], invoked: &mut Vec<Name>, at: &mut Vec<u32>, names: &mut Vec<Name>| {
        for (x, y) in region_closure(&regions, segs) {
            let t = pf.bytes(x, y);
            scan(t, &[(0, t.len() as u32)], &[], names, &mut Vec::new());
        }
        // an invocation's expansion is called from its path's start (at most a long path before
        // the name the scan saw)
        // an invocation inside the arguments of an expanded one is expanded within that expansion
        // (if it is used at all), whose names are in: it needs no expansion of its own here
        let spans: Vec<(u32, u32)> = at.chunks(2).map(|c| (c[0], c.get(1).copied().unwrap_or(c[0]))).collect();
        let direct: Vec<bool> = spans.iter().map(|&(p, _)| {
            let k = regions.partition_point(|r| r.0 + 256 < p);
            regions[k..].iter().take_while(|r| r.0 <= p).any(|r| r.0 + 256 >= p)
        }).collect();
        let (mut keep, mut keep_at) = (Vec::new(), Vec::new());
        for (i, m) in invoked.iter().enumerate() {
            let (p, _) = spans[i];
            let inside = spans.iter().zip(&direct).any(|(&(a, b), &d)| d && a < p && p < b);
            if !direct[i] && !inside { keep.push(*m); keep_at.push(p); }
        }
        *invoked = keep;
        *at = keep_at;
    };
    // the tokens of the file, for brace positions
    let brace_at_or_after = |from: u32, to: u32, last: bool| -> Option<u32> {
        let t0 = pf.toks.partition_point(|t| t.lo < from);
        let mut found = None;
        for t in pf.toks.iter_from(t0) {
            if t.lo >= to { break; }
            if t.kind == crate::lexer::T::OpenBrace {
                found = Some(t.hi);
                if !last { break; }
            }
        }
        found
    };
    // the file's inner attributes: seen by every node of the crate
    {
        let mut segs = Vec::new();
        let root_attrs: Vec<crate::ast::Attr> = ast.root_attrs.iter().copied().collect();
        attr_segs(pf, &root_attrs, &mut segs);
        if !segs.is_empty() {
            let (mut names, mut invoked) = (Vec::new(), Vec::new());
            scan(src, &segs, &[], &mut names, &mut invoked);
            out.push(Raw { items: Vec::new(), piece: piece_hash(src, 0, &segs), names, keys: Vec::new(), invoked, invoked_at: Vec::new(), always: true, mac: false, rules: false, header: None, trait_name: None, named_head: true });
        }
    }
    let items = reached(pf);
    let parent_of: HashMap<u32, Option<u32>> = items.iter().copied().collect();
    let kind_of = |i: u32| &ast.item(crate::ast::ItemId(i)).kind;
    // an item's identity: its file, the inline modules around it, its kind and name -- two items
    // with the same text are two nodes (a new `fn which` beside a glob-imported one changes what
    // the name finds), and none moves when text above it does
    let ident = |i: u32| -> u128 {
        let mut h = Sip128::new();
        h.str("dep-ident");
        h.str(path);
        let mut at = parent_of.get(&i).copied().flatten();
        let mut depth = 0;
        while let Some(p) = at {
            if depth > 64 { break; }
            let pi = ast.item(crate::ast::ItemId(p));
            h.u32(kind_code(&pi.kind));
            h.str(pf.text(pi.name));
            at = parent_of.get(&p).copied().flatten();
            depth += 1;
        }
        h.u8(0xfd);
        let it = ast.item(crate::ast::ItemId(i));
        h.u32(kind_code(&it.kind));
        h.str(pf.text(it.name));
        h.finish128()
    };
    // whole-block nodes (traits, trait impls): their items are theirs
    let whole = |i: u32| -> bool {
        match kind_of(i) {
            ItemKind::Trait(_) => true,
            ItemKind::Impl(r) => ast.row(*r).trait_.is_some(),
            _ => false,
        }
    };
    let name_span = |i: u32| -> Option<(u32, u32)> {
        let it = ast.item(crate::ast::ItemId(i));
        (it.name.hi > it.name.lo).then_some((it.name.lo, it.name.hi))
    };
    for &(i, parent) in &items {
        // an item of a whole-block node is part of it
        if parent.is_some_and(whole) { continue; }
        let it = ast.item(crate::ast::ItemId(i));
        let (lo, hi) = (it.lo, it.hi);
        let mut segs = Vec::new();
        attr_segs(pf, ast.list(it.attrs), &mut segs);
        segs.push((lo, hi));
        let mut cut: Vec<(u32, u32)> = all_holes.iter().copied().filter(|&(a, b)| a >= lo && b <= hi).collect();
        let mut skip: Vec<(u32, u32)> = name_span(i).into_iter().collect();
        let mut keys = Vec::new();
        let mut members = vec![i];
        let mut always = false;
        let (mut mac, mut rules) = (false, false);
        let (mut trait_name, mut named_head) = (None, true);
        let name = pf.text(it.name);
        let own = |keys: &mut Vec<Name>| if !name.is_empty() { keys.push(name_of(name.as_bytes())); };
        // a container's header: up to the brace that opens its items (a module's or extern block's
        // first brace; an impl's last brace before its first item, so a brace in its generics stays
        // in), and the inner attributes after it. The text between the items is theirs, or item
        // macro invocations (their expansions are items of their own) and comments.
        let header_only = |cut: &mut Vec<(u32, u32)>, kids: &[crate::ast::ItemId], impl_: bool| {
            let first = kids.iter().map(|k| full_span(pf, k.0).0).filter(|&x| x >= lo && x < hi).min().unwrap_or(hi);
            let brace = if impl_ { brace_at_or_after(lo, first, true) } else { brace_at_or_after(lo, hi, false) };
            match brace {
                Some(b) if b <= first || !impl_ => {
                    // inner attributes: `#![...]` between the brace and the first item
                    let t0 = pf.toks.partition_point(|t| t.lo < b);
                    let mut keep_to = b;
                    let toks: Vec<_> = pf.toks.iter_from(t0).take_while(|t| t.lo < first.min(hi)).collect();
                    let mut i = 0;
                    while i + 2 < toks.len() && toks[i].kind == crate::lexer::T::Pound && toks[i + 1].kind == crate::lexer::T::Not && toks[i + 2].kind == crate::lexer::T::OpenBracket {
                        let mut depth = 0i32;
                        let mut j = i + 2;
                        while j < toks.len() {
                            match toks[j].kind { crate::lexer::T::OpenBracket => depth += 1, crate::lexer::T::CloseBracket => depth -= 1, _ => {} }
                            if depth == 0 { break; }
                            j += 1;
                        }
                        if j >= toks.len() { break; }
                        keep_to = toks[j].hi;
                        i = j + 1;
                    }
                    cut.push((keep_to, hi));
                }
                _ => { for k in kids { cut.push(full_span(pf, k.0)); } }
            }
        };
        let header = parent.filter(|&p| matches!(kind_of(p), ItemKind::Impl(_) | ItemKind::ForeignMod(_)));
        match &it.kind {
            ItemKind::Mod(Some(kids)) => { header_only(&mut cut, ast.list(*kids), false); own(&mut keys); }
            ItemKind::ForeignMod(r) => header_only(&mut cut, ast.list(ast.row(*r).items), false),
            ItemKind::Impl(r) => {
                let im = ast.row(*r);
                if im.trait_.is_none() {
                    // an inherent impl's header: reached from its items
                    header_only(&mut cut, ast.list(im.items), true);
                } else {
                    for k in ast.list(im.items) {
                        members.push(k.0);
                        if let Some(s) = name_span(k.0) { skip.push(s); }
                    }
                    let params: Vec<&str> = ast.list(im.generics.params).iter().map(|g| pf.text(g.name)).collect();
                    named_head = ty_names(pf, im.self_ty, &params, &mut keys, 0);
                    trait_name = im.trait_.as_ref().and_then(|p| ast.segs(p.segs).last()).map(|s| pf.text(s.name)).filter(|t| !t.is_empty()).map(|t| name_of(t.as_bytes()));
                }
            }
            ItemKind::Trait(r) => {
                own(&mut keys);
                for k in ast.list(ast.row(*r).items) {
                    members.push(k.0);
                    let kn = pf.text(ast.item(*k).name);
                    if !kn.is_empty() { keys.push(name_of(kn.as_bytes())); }
                    if let Some(s) = name_span(k.0) { skip.push(s); }
                }
            }
            ItemKind::Enum(r) => {
                own(&mut keys);
                for v in ast.list(ast.row(*r).variants) {
                    let vn = pf.text(v.name);
                    if !vn.is_empty() { keys.push(name_of(vn.as_bytes())); }
                    if v.name.hi > v.name.lo { skip.push((v.name.lo, v.name.hi)); }
                }
            }
            ItemKind::Use(r) => {
                skip.clear();
                let u = ast.row(*r);
                use_names(pf, &u.tree, None, &mut keys, &mut always, 0);
            }
            ItemKind::ExternCrate(alias) => {
                skip.clear();
                match alias { Some(a) => keys.push(name_of(pf.text(*a).as_bytes())), None => own(&mut keys) }
            }
            ItemKind::Mac(_) => { always = true; mac = true; }
            ItemKind::MacroRules(_) => { own(&mut keys); rules = true; }
            _ => own(&mut keys),
        }
        let segs = minus(segs, &mut cut);
        let (mut names, mut invoked, mut at) = (Vec::new(), Vec::new(), Vec::new());
        scan_at(src, &segs, &skip, &mut names, &mut invoked, &mut at);
        if !rules { expanded(&segs, &mut invoked, &mut at, &mut names); }
        names.sort_unstable();
        names.dedup();
        keys.sort_unstable();
        keys.dedup();
        let mut ph = Sip128::new();
        ph.u128(ident(i));
        ph.u128(piece_hash(src, kind_code(&it.kind), &segs));
        out.push(Raw { items: members, piece: ph.finish128(), names, keys, invoked, invoked_at: at, always, mac, rules, header, trait_name, named_head });
    }
    out
}

fn kind_code(k: &ItemKind) -> u32 {
    match k {
        ItemKind::Use(_) => 1, ItemKind::ExternCrate(_) => 2, ItemKind::Fn(..) => 3, ItemKind::Struct(_) => 4,
        ItemKind::Union(_) => 5, ItemKind::Enum(_) => 6, ItemKind::TypeAlias(_) => 7, ItemKind::Const(..) => 8,
        ItemKind::Static(..) => 9, ItemKind::Trait(_) => 10, ItemKind::TraitAlias(_) => 11, ItemKind::Impl(_) => 12,
        ItemKind::Mod(_) => 13, ItemKind::ForeignMod(_) => 14, ItemKind::MacroRules(_) => 15, ItemKind::Mac(_) => 16,
    }
}

impl Deps {
    /// Per crate: what its rows are built from, besides the crates it can see (CrateRow without its
    /// own lists and always hash).
    fn crate_inputs(prog: &Program, iface: &[u128], atomic: &[bool]) -> Vec<CrateRow> {
        let mut files: Vec<Sip128> = (0..prog.crates.len()).map(|_| { let mut h = Sip128::new(); h.str("deps-crate-files"); h }).collect();
        for f in 0..prog.boot_files.min(prog.files.len()) {
            if prog.front_macro_fragments.contains(&f) { continue; }
            let Some(&c) = prog.file_crate.get(f) else { continue };
            let Some(h) = files.get_mut(c as usize) else { continue };
            h.u32(f as u32);
            h.str(prog.file_paths.get(f).map_or("", |p| p.as_str()));
            h.u32(prog.files[f].ast.items.len() as u32);
        }
        files.into_iter().enumerate().map(|(c, files)| {
            let ci = &prog.crates[c];
            let mut ident = Sip128::new();
            ident.str("crate-ident");
            ident.str(prog.syms.str(ci.name));
            ident.str(prog.mods.get(ci.root_mod as usize).and_then(|m| prog.file_paths.get(m.file as usize)).map_or("", |p| p.as_str()));
            CrateRow { ident: h2(ident.finish128()), files: h2(files.finish128()),
                iface: h2(iface.get(c).copied().unwrap_or(0)), always: [0; 2], always_at: [0; 2], aliases_at: [0; 2], macros_at: [0; 2],
                atomic: atomic.get(c).copied().unwrap_or(true) as u32, _pad: 0 }
        }).collect()
    }

    /// Per crate: whether its rows in `prev` stand for this program's -- its inputs and those of every
    /// crate it can see are as `prev` was built from (a graph's rows of a crate depend only on them).
    fn clean_with(prev: Option<&Deps>, inputs: &[CrateRow], universe: &[Vec<u64>]) -> Vec<bool> {
        let n = inputs.len();
        let Some(prev) = prev else { return vec![false; n] };
        let rows = prev.crates.rows();
        let same = |a: &CrateRow, b: &CrateRow| (a.ident, a.files, a.iface, a.atomic) == (b.ident, b.files, b.iface, b.atomic);
        let own: Vec<bool> = (0..n).map(|c| rows.get(c).is_some_and(|r| same(r, &inputs[c]))).collect();
        (0..n).map(|c| (0..n).all(|d| !bit(&universe[c], d as u32) || own[d])).collect()
    }

    /// The crates whose rows a build over `prev` takes over (`build`): their files need no digests.
    pub(crate) fn clean_crates(prog: &Program, iface: &[u128], prev: Option<&Deps>) -> Vec<bool> {
        let atomic = atomic_crates(prog, iface.len());
        Self::clean_with(prev, &Self::crate_inputs(prog, iface, &atomic), &universes(prog))
    }

    /// Builds the graph of the program's boot files; `iface`: the crates' interface fingerprints
    /// (what coarse nodes and sysroot crates are keyed by), `valid` what
    /// it is built from (Deps::valid). Over `prev` (the graph a start before this one stored), the
    /// rows of every crate that is clean (`clean_crates`) are taken over and only the other crates'
    /// files are walked: their nodes' edges into clean crates end at those crates' name rows, whose
    /// closures and components are known. The result is the graph a build from scratch makes (the
    /// component ids aside). None: over `prev`, the names that decide clean crates' nodes (the
    /// program's alias and macro names) changed -- build without it.
    pub(crate) fn build(prog: &Program, digests: &[Option<std::sync::Arc<super::items::FileDigest>>], iface: &[u128], valid: u128, prev: Option<&Deps>) -> Option<Deps> {
        let t0 = std::time::Instant::now();
        let n_crates = prog.crates.len();
        let atomic = atomic_crates(prog, iface.len());
        let universe = universes(prog);
        let mut crates = Self::crate_inputs(prog, iface, &atomic);
        let clean = Self::clean_with(prev, &crates, &universe);
        let prev = prev.filter(|_| clean.iter().any(|&c| c));
        // the files walked: boot files of the crates in the graph that are not clean
        let files: Vec<usize> = (0..prog.boot_files.min(prog.files.len()))
            .filter(|&f| !prog.front_macro_fragments.contains(&f))
            .filter(|&f| prog.file_crate.get(f).is_some_and(|&c| !atomic.get(c as usize).copied().unwrap_or(true) && !clean[c as usize]))
            .filter(|&f| digests.get(f).is_some_and(|d| d.is_some()))
            .collect();
        // a fn body in the interface (an `impl Trait` return, async, const, items inside) that the
        // load skipped and that invokes a macro: its file's skipped bodies are parsed now (the
        // expansions' names belong to the node; without them it would be coarse -- and whether it
        // is must not depend on what ran before the graph was built)
        for &f in &files {
            let (Some(pf), Some(Some(dg))) = (prog.files.get(f), digests.get(f)) else { continue };
            if pf.part.get().is_some() { continue; }
            let needs = pf.ast.items.iter().enumerate().any(|(i, it)| {
                matches!(it.kind, ItemKind::Fn(sig, None, _) if pf.ast.fn_sig(sig).lazy_body.is_some())
                    && dg.body_span(crate::ast::ItemId(i as u32)).is_none()
                    && { let (mut n, mut inv) = (Vec::new(), Vec::new()); scan(&pf.src, &[(it.lo, it.hi)], &[], &mut n, &mut inv); inv.iter().any(|m| m & QUIET_BIT == 0) }
            });
            if needs {
                if let Some(&c) = prog.file_crate.get(f) { let _ = pf.part(&prog.crates[c as usize].cfg); }
            }
        }
        struct Shared<'a>(&'a [std::sync::Arc<ParsedFile>]);
        unsafe impl Sync for Shared<'_> {}
        let pfs = Shared(&prog.files);
        let paths = &prog.file_paths;
        let threads = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 8);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let per_file: Vec<Vec<(usize, Vec<Raw>)>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads).map(|_| {
                let (pfs, next, files, paths) = (&pfs, &next, &files, paths);
                s.spawn(move || {
                    let mut v = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&f) = files.get(k) else { break };
                        let Some(Some(dg)) = digests.get(f) else { continue };
                        v.push((f, file_nodes(&pfs.0[f], dg, paths.get(f).map_or("", |p| p.as_str()))));
                    }
                    v
                })
            }).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        let t_files = t0.elapsed();
        let mut per_file: Vec<(usize, Vec<Raw>)> = per_file.into_iter().flatten().collect();
        per_file.sort_unstable_by_key(|x| x.0);
        // number the nodes; map items to them
        let mut raws: Vec<Raw> = Vec::new();
        let mut node_crate: Vec<u32> = Vec::new();
        let mut node_file: Vec<u32> = Vec::new();
        let mut node_of: HashMap<(u32, u32), u32> = HashMap::new();
        for (f, nodes) in per_file {
            let c = prog.file_crate[f];
            for r in nodes {
                let id = raws.len() as u32;
                for &i in &r.items { node_of.insert((f as u32, i), id); }
                node_crate.push(c);
                node_file.push(f as u32);
                raws.push(r);
            }
        }
        // the names of aliases and of user macros, by crate (a clean crate's from `prev`)
        let mut own_aliases: Vec<Vec<Name>> = vec![Vec::new(); n_crates];
        let mut own_macros: Vec<Vec<Name>> = vec![Vec::new(); n_crates];
        for (id, r) in raws.iter().enumerate() {
            let f = node_file[id] as usize;
            if let Some(&i) = r.items.first() {
                match &prog.files[f].ast.item(crate::ast::ItemId(i)).kind {
                    ItemKind::TypeAlias(_) => own_aliases[node_crate[id] as usize].extend(r.keys.iter().copied()),
                    ItemKind::MacroRules(_) => own_macros[node_crate[id] as usize].extend(r.keys.iter().copied()),
                    _ => {}
                }
            }
        }
        // proc macros: the fns of the proc-macro crates' root files (their names)
        for (f, pf) in prog.files.iter().enumerate().take(prog.boot_files.min(prog.files.len())) {
            let Some(&c) = prog.file_crate.get(f) else { continue };
            if clean[c as usize] || !prog.crates.get(c as usize).is_some_and(|ci| ci.proc_macro) { continue; }
            for &i in pf.ast.root_items.iter() {
                let it = pf.ast.item(i);
                if matches!(it.kind, ItemKind::Fn(..)) { own_macros[c as usize].push(name_of(pf.text(it.name).as_bytes())); }
            }
        }
        if let Some(prev) = prev {
            let (rows, lists) = (prev.crates.rows(), prev.lists.rows());
            let list = |at: [u32; 2]| &lists[at[0] as usize..(at[0] + at[1]) as usize];
            for c in (0..n_crates).filter(|&c| clean[c]) {
                own_aliases[c] = list(rows[c].aliases_at).to_vec();
                own_macros[c] = list(rows[c].macros_at).to_vec();
            }
        }
        for v in own_aliases.iter_mut().chain(own_macros.iter_mut()) { v.sort_unstable(); v.dedup(); }
        let aliases: std::collections::HashSet<Name> = own_aliases.iter().flatten().copied().collect();
        let user_macros: std::collections::HashSet<Name> = own_macros.iter().flatten().copied().collect();
        if let Some(prev) = prev {
            // a clean crate's nodes were made with the program's alias and macro names of then
            let (rows, lists) = (prev.crates.rows(), prev.lists.rows());
            let was: std::collections::HashSet<Name> = rows.iter().flat_map(|r| lists[r.aliases_at[0] as usize..(r.aliases_at[0] + r.aliases_at[1]) as usize].iter().copied()).collect();
            if was != aliases || prev.macros.rows().len() != user_macros.len() || prev.macros.rows().iter().any(|m| !user_macros.contains(m)) { return None; }
        }
        // a trait impl for a type named nowhere (a parameter, a primitive, a reference, ...) or for
        // an alias (another type) is also found under its trait's name
        for r in raws.iter_mut() {
            if let Some(t) = r.trait_name {
                if !r.named_head || r.keys.iter().any(|k| aliases.contains(k)) {
                    r.keys.push(t);
                    r.keys.sort_unstable();
                    r.keys.dedup();
                }
            }
        }
        // the nodes found under each name, by crate: one name node per (name, crate) stands for
        // them (a node naming a name depends on that name's nodes in each crate it can see)
        let mut index: HashMap<Name, Vec<u32>> = HashMap::new();
        for (id, r) in raws.iter().enumerate() {
            for &k in &r.keys { index.entry(k).or_default().push(id as u32); }
        }
        let n_items = raws.len();
        let mut name_nodes: HashMap<Name, Vec<(u32, u32)>> = HashMap::with_capacity(index.len());
        let mut nn_targets: Vec<Vec<u32>> = Vec::new();
        let mut nn_name: Vec<(Name, u32)> = Vec::new();
        for (&name, ids) in index.iter_mut() {
            ids.sort_unstable_by_key(|&t| (node_crate[t as usize], t));
            let mut v: Vec<(u32, u32)> = Vec::new();
            let mut k = 0;
            while k < ids.len() {
                let c = node_crate[ids[k] as usize];
                let mut e = k;
                while e < ids.len() && node_crate[ids[e] as usize] == c { e += 1; }
                v.push((c, (n_items + nn_targets.len()) as u32));
                nn_targets.push(ids[k..e].to_vec());
                nn_name.push((name, c));
                k = e;
            }
            name_nodes.insert(name, v);
        }
        let n_named = n_items + nn_targets.len();
        // a node invoking a macro outside the item position: coarse
        let mut coarse = vec![false; n_items];
        let mut ncoarse = 0;
        for (id, r) in raws.iter().enumerate() {
            let c = r.mac || (!r.rules && r.invoked.iter().any(|&m| m & QUIET_BIT == 0 || user_macros.contains(&(m & !QUIET_BIT))));
            if c { coarse[id] = true; ncoarse += 1; }
        }
        // edges (CSR), the item nodes' on several threads; an edge into a clean crate's name row is
        // that row's index with LEAF set (the rows' components are the leaves below)
        const LEAF: u32 = 1 << 31;
        let prev_names: &[NameRow] = prev.map_or(&[], |p| p.names.rows());
        let header_node = |id: usize| -> Option<u32> {
            let h = raws[id].header?;
            node_of.get(&(node_file[id], h)).copied()
        };
        let chunk = n_items.div_ceil(threads).max(1);
        let parts: Vec<(Vec<u32>, Vec<u32>)> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..threads).map(|t| {
                let (raws, coarse, universe, node_crate, name_nodes, header_node, clean) = (&raws, &coarse, &universe, &node_crate, &name_nodes, &header_node, &clean);
                s.spawn(move || {
                    let (lo, hi) = ((t * chunk).min(n_items), ((t + 1) * chunk).min(n_items));
                    let mut lens = Vec::with_capacity(hi - lo);
                    let mut tgt = Vec::new();
                    for id in lo..hi {
                        let start = tgt.len();
                        if !coarse[id] {
                            let uni = &universe[node_crate[id] as usize];
                            for nm in &raws[id].names {
                                if let Some(v) = name_nodes.get(nm) {
                                    for &(c, nn) in v { if bit(uni, c) { tgt.push(nn); } }
                                }
                                if !prev_names.is_empty() {
                                    let at = prev_names.partition_point(|r| r.name < *nm);
                                    for (k, r) in prev_names[at..].iter().enumerate().take_while(|(_, r)| r.name == *nm) {
                                        if bit(uni, r.krate) && clean[r.krate as usize] { tgt.push(LEAF | (at + k) as u32); }
                                    }
                                }
                            }
                            if let Some(h) = header_node(id) { tgt.push(h); }
                            tgt[start..].sort_unstable();
                            let mut w = start;
                            for r in start..tgt.len() {
                                if r == start || tgt[r] != tgt[w - 1] { tgt[w] = tgt[r]; w += 1; }
                            }
                            tgt.truncate(w);
                        }
                        lens.push((tgt.len() - start) as u32);
                    }
                    (lens, tgt)
                })
            }).collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        // the leaves: one node per component of the clean rows named (its closure is the row's)
        let mut leaf_of: HashMap<u32, u32> = HashMap::new();
        let mut leaves: Vec<u128> = Vec::new();
        let mut off: Vec<u32> = Vec::with_capacity(n_named + 1);
        let mut tgt: Vec<u32> = Vec::new();
        off.push(0);
        for (lens, t) in parts {
            let mut at = 0usize;
            for l in lens {
                let start = tgt.len();
                for &x in &t[at..at + l as usize] {
                    if x & LEAF == 0 { tgt.push(x); continue; }
                    let r = &prev_names[(x & !LEAF) as usize];
                    let leaf = *leaf_of.entry(r.comp).or_insert_with(|| { leaves.push(u2(r.closure)); (leaves.len() - 1) as u32 });
                    tgt.push(n_named as u32 + leaf);
                }
                if tgt.len() - start > 1 {
                    tgt[start..].sort_unstable();
                    let mut w = start;
                    for r in start..tgt.len() {
                        if r == start || tgt[r] != tgt[w - 1] { tgt[w] = tgt[r]; w += 1; }
                    }
                    tgt.truncate(w);
                }
                off.push(tgt.len() as u32);
                at += l as usize;
            }
        }
        for ts in &nn_targets {
            tgt.extend_from_slice(ts);
            off.push(tgt.len() as u32);
        }
        let n = n_named + leaves.len();
        off.resize(n + 1, tgt.len() as u32);
        let edges = tgt.len();
        let t_edges = t0.elapsed();
        // strongly connected components (iterative Tarjan); components come out in reverse
        // topological order: every component's successors are done before it
        let mut comp = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut num = vec![u32::MAX; n];
        let mut on = vec![false; n];
        let mut st: Vec<u32> = Vec::new();
        let mut comps: Vec<Vec<u32>> = Vec::new();
        let mut counter = 0u32;
        let mut call: Vec<(u32, u32)> = Vec::new();
        for root in 0..n as u32 {
            if num[root as usize] != u32::MAX { continue; }
            call.push((root, off[root as usize]));
            num[root as usize] = counter; low[root as usize] = counter; counter += 1;
            st.push(root); on[root as usize] = true;
            while let Some(&mut (v, ref mut e)) = call.last_mut() {
                let vi = v as usize;
                if *e < off[vi + 1] {
                    let w = tgt[*e as usize];
                    *e += 1;
                    let wi = w as usize;
                    if num[wi] == u32::MAX {
                        num[wi] = counter; low[wi] = counter; counter += 1;
                        st.push(w); on[wi] = true;
                        call.push((w, off[wi]));
                    } else if on[wi] {
                        low[vi] = low[vi].min(num[wi]);
                    }
                } else {
                    call.pop();
                    if let Some(&(p, _)) = call.last() { low[p as usize] = low[p as usize].min(low[vi]); }
                    if low[vi] == num[vi] {
                        let k = comps.len() as u32;
                        let mut members = Vec::new();
                        while let Some(x) = st.pop() {
                            on[x as usize] = false;
                            comp[x as usize] = k;
                            members.push(x);
                            if x == v { break; }
                        }
                        comps.push(members);
                    }
                }
            }
        }
        let t_scc = t0.elapsed();
        let mut chash = vec![0u128; comps.len()];
        let mut closure = vec![0u128; n];
        let mut succ_c: Vec<u32> = Vec::new();
        for (k, members) in comps.iter().enumerate() {
            // a leaf (it has no edges: its own component) is the clean component it stands for
            if let [m] = members[..] {
                if m as usize >= n_named {
                    chash[k] = leaves[m as usize - n_named];
                    closure[m as usize] = chash[k];
                    continue;
                }
            }
            let mut own: Vec<u128> = Vec::with_capacity(members.len());
            succ_c.clear();
            for &m in members {
                let mi = m as usize;
                let mut h = Sip128::new();
                h.u128(if mi < n_items { raws[mi].piece } else { 0 });
                if mi < n_items && coarse[mi] {
                    h.str("coarse");
                    h.u128(iface.get(node_crate[mi] as usize).copied().unwrap_or(0));
                }
                // (a name node is its name's nodes: no piece of its own, its members are its edges)
                own.push(h.finish128());
                for &t in &tgt[off[mi] as usize..off[mi + 1] as usize] {
                    let tc = comp[t as usize];
                    if tc as usize != k { succ_c.push(tc); }
                }
            }
            succ_c.sort_unstable();
            succ_c.dedup();
            own.sort_unstable();
            // the successor components as a multiset of their hashes: summed (a sum of independent
            // 128-bit hashes; no sort of the hashes)
            let sum = succ_c.iter().fold(0u128, |a, &c| a.wrapping_add(chash[c as usize]));
            let mut h = Sip128::new();
            h.str("dep-scc");
            h.u32(own.len() as u32);
            for x in &own { h.u128(*x); }
            h.u32(succ_c.len() as u32);
            h.u128(sum);
            chash[k] = h.finish128();
            for &m in members { closure[m as usize] = chash[k]; }
        }
        // per crate: its always nodes' closures (a clean crate's from `prev`), and the closures of
        // the always nodes of every crate it can see
        let mut always_of: Vec<Vec<u128>> = vec![Vec::new(); n_crates];
        for (id, r) in raws.iter().enumerate() {
            if r.always { always_of[node_crate[id] as usize].push(closure[id]); }
        }
        if let Some(prev) = prev {
            let (rows, lists) = (prev.crates.rows(), prev.lists.rows());
            for c in (0..n_crates).filter(|&c| clean[c]) {
                let at = rows[c].always_at;
                always_of[c] = lists[at[0] as usize..(at[0] + at[1]) as usize].chunks(2).map(|w| u2([w[0], w[1]])).collect();
            }
        }
        for v in always_of.iter_mut() { v.sort_unstable(); }
        // (the crates it can see by identity, not by load-order index: a load order that changes
        // keeps the keys of code that did not)
        let mut by_ident: Vec<usize> = (0..n_crates).filter(|&k| !always_of[k].is_empty()).collect();
        by_ident.sort_unstable_by_key(|&k| crates[k].ident);
        for c in 0..n_crates {
            crates[c].always = match prev {
                Some(prev) if clean[c] => prev.crates.rows()[c].always,
                _ => {
                    let mut h = Sip128::new();
                    h.str("dep-always-v2");
                    for &k in by_ident.iter().filter(|&&k| bit(&universe[c], k as u32)) {
                        h.u64(crates[k].ident[0]); h.u64(crates[k].ident[1]);
                        for x in &always_of[k] { h.u128(*x); }
                    }
                    h2(h.finish128())
                }
            };
        }
        let mut lists: Vec<u64> = Vec::new();
        for c in 0..n_crates {
            let at = |lists: &Vec<u64>, start: usize| [start as u32, (lists.len() - start) as u32];
            let s = lists.len();
            for x in &always_of[c] { lists.extend_from_slice(&h2(*x)); }
            crates[c].always_at = at(&lists, s);
            let s = lists.len();
            lists.extend_from_slice(&own_aliases[c]);
            crates[c].aliases_at = at(&lists, s);
            let s = lists.len();
            lists.extend_from_slice(&own_macros[c]);
            crates[c].macros_at = at(&lists, s);
        }
        // MAKEPAD_RUN_DEPS_DUMP=<file>: every item node walked (crate, file:offset, piece, closure,
        // coarse and the macros that made it so): which piece an edit changed, and what reaches it
        if let Ok(path) = std::env::var("MAKEPAD_RUN_DEPS_DUMP") {
            let mut out = String::new();
            for id in 0..n_items {
                let f = node_file[id] as usize;
                let at = raws[id].items.first().map(|&i| prog.files[f].ast.item(crate::ast::ItemId(i)).lo).unwrap_or(0);
                let (l, _) = prog.files[f].line_col(at);
                let keys: Vec<String> = raws[id].keys.iter().map(|k| format!("{:x}", k)).collect();
                let why = if !coarse[id] { "0".to_string() } else if raws[id].mac { "1".to_string() } else {
                    let lines: Vec<String> = raws[id].invoked.iter().zip(&raws[id].invoked_at)
                        .filter(|(m, _)| **m & QUIET_BIT == 0 || user_macros.contains(&(**m & !QUIET_BIT)))
                        .map(|(_, &p)| { let (l, c) = prog.files[f].line_col(p); format!("{}:{}", l, c) }).collect();
                    format!("2@{}", lines.join(","))
                };
                out.push_str(&format!("{}\t{}\t{}:{}\t{:032x}\t{:032x}\t{}\t{}\t{}\n", id, prog.syms.str(prog.crates[node_crate[id] as usize].name),
                    prog.file_paths.get(f).map_or("", |p| p.as_str()), l, raws[id].piece, closure[id], why, off[id + 1] - off[id], keys.join(",")));
            }
            let _ = std::fs::write(format!("{}.{}", path, std::process::id()), out);
        }
        // the rows: the clean crates' from `prev`, the walked ones' new (their components numbered
        // after every id `prev` uses)
        let base = prev.map_or(0, |p| p.comp_end);
        let clean_file = |f: u32| prog.file_crate.get(f as usize).is_some_and(|&c| clean[c as usize]);
        let mut nodes: Vec<NodeRow> = prev.map_or(&[][..], |p| p.nodes.rows()).iter().filter(|r| clean_file(r.file)).copied().collect();
        let taken = nodes.len();
        nodes.extend(node_of.iter().map(|(&(file, item), &id)| NodeRow { file, item, closure: h2(closure[id as usize]) }));
        nodes.sort_unstable_by_key(|r| (r.file, r.item));
        let mut names: Vec<NameRow> = prev_names.iter().filter(|r| clean.get(r.krate as usize).copied().unwrap_or(false)).copied().collect();
        names.extend(nn_name.iter().enumerate().map(|(i, &(name, krate))| {
            let nn = n_items + i;
            NameRow { name, krate, comp: base + comp[nn], closure: h2(closure[nn]) }
        }));
        names.sort_unstable_by_key(|r| (r.name, r.krate));
        let mut macros: Vec<u64> = user_macros.into_iter().collect();
        macros.sort_unstable();
        let phases = [t_files, t_edges, t_scc, t0.elapsed()].map(|d| d.as_secs_f64() * 1e3);
        Some(Deps { valid, nodes_n: nodes.len(), walked: nodes.len() - taken, nodes: Rows::Owned(nodes), names: Rows::Owned(names), macros: Rows::Owned(macros),
            crates: Rows::Owned(crates), lists: Rows::Owned(lists), comp_end: base + comps.len() as u32, universe, atomic, _store: None,
            edges, coarse: ncoarse, phases })
    }

    /// The graph as a store entry's payload.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let (crates, lists, macros, nodes, names) = (self.crates.rows(), self.lists.rows(), self.macros.rows(), self.nodes.rows(), self.names.rows());
        let mut p = Vec::with_capacity(48 + crates.len() * std::mem::size_of::<CrateRow>() + (lists.len() + macros.len()) * 8 + nodes.len() * 24 + names.len() * 32);
        p.extend_from_slice(DEPS_MAGIC);
        p.extend_from_slice(&DEPS_VERSION.to_le_bytes());
        p.extend_from_slice(&self.valid.to_le_bytes());
        p.extend_from_slice(&self.comp_end.to_le_bytes());
        for n in [crates.len(), lists.len(), macros.len(), nodes.len(), names.len()] { p.extend_from_slice(&(n as u32).to_le_bytes()); }
        for r in crates {
            for w in [r.ident, r.files, r.iface, r.always] { for x in w { p.extend_from_slice(&x.to_le_bytes()); } }
            for w in [r.always_at, r.aliases_at, r.macros_at] { for x in w { p.extend_from_slice(&x.to_le_bytes()); } }
            p.extend_from_slice(&r.atomic.to_le_bytes());
            p.extend_from_slice(&0u32.to_le_bytes());
        }
        for x in lists.iter().chain(macros) { p.extend_from_slice(&x.to_le_bytes()); }
        for r in nodes {
            p.extend_from_slice(&r.file.to_le_bytes());
            p.extend_from_slice(&r.item.to_le_bytes());
            p.extend_from_slice(&r.closure[0].to_le_bytes());
            p.extend_from_slice(&r.closure[1].to_le_bytes());
        }
        for r in names {
            p.extend_from_slice(&r.name.to_le_bytes());
            p.extend_from_slice(&r.krate.to_le_bytes());
            p.extend_from_slice(&r.comp.to_le_bytes());
            p.extend_from_slice(&r.closure[0].to_le_bytes());
            p.extend_from_slice(&r.closure[1].to_le_bytes());
        }
        p
    }

    /// A graph stored by `encode`, read in place (`store` holds the mapping of `p`), for this
    /// program (`Deps::valid` says whether it was built from it). None: another format, a size that
    /// does not match, or a misaligned payload.
    pub(crate) fn read(prog: &Program, ncr: usize, p: &'static [u8], store: std::sync::Arc<crate::store::Store>) -> Option<Deps> {
        const HEAD: usize = 48;
        if cfg!(target_endian = "big") || p.len() < HEAD || &p[0..4] != DEPS_MAGIC || u32::from_le_bytes(p[4..8].try_into().ok()?) != DEPS_VERSION { return None; }
        let valid = u128::from_le_bytes(p[8..24].try_into().ok()?);
        let word = |at: usize| u32::from_le_bytes(p[at..at + 4].try_into().unwrap());
        let comp_end = word(24);
        let (nc, nl, nm, nn, nx) = (word(28) as usize, word(32) as usize, word(36) as usize, word(40) as usize, word(44) as usize);
        let c0 = HEAD;
        let l0 = c0 + nc * std::mem::size_of::<CrateRow>();
        let m0 = l0 + nl * 8;
        let n0 = m0 + nm * 8;
        let x0 = n0 + nn * 24;
        if std::mem::size_of::<CrateRow>() != 96 || p.len() != x0 + nx * 32 || p.as_ptr() as usize % 8 != 0 { return None; }
        // SAFETY: the payload stays mapped while `store` lives (held below); the tables are
        // 8-byte aligned (checked), plain integers, and their lengths match the payload's size
        let crates: &'static [CrateRow] = unsafe { std::slice::from_raw_parts(p.as_ptr().add(c0) as *const CrateRow, nc) };
        let lists: &'static [u64] = unsafe { std::slice::from_raw_parts(p.as_ptr().add(l0) as *const u64, nl) };
        let macros: &'static [u64] = unsafe { std::slice::from_raw_parts(p.as_ptr().add(m0) as *const u64, nm) };
        let nodes: &'static [NodeRow] = unsafe { std::slice::from_raw_parts(p.as_ptr().add(n0) as *const NodeRow, nn) };
        let names: &'static [NameRow] = unsafe { std::slice::from_raw_parts(p.as_ptr().add(x0) as *const NameRow, nx) };
        // every list range in bounds (a build over this graph slices them)
        if crates.iter().any(|r| [r.always_at, r.aliases_at, r.macros_at].iter().any(|at| at[0] as usize + at[1] as usize > nl) || r.always_at[1] % 2 != 0) { return None; }
        Some(Deps { valid, nodes: Rows::Mapped(nodes), names: Rows::Mapped(names), macros: Rows::Mapped(macros), crates: Rows::Mapped(crates),
            lists: Rows::Mapped(lists), comp_end, universe: universes(prog), atomic: atomic_crates(prog, ncr), _store: Some(store),
            nodes_n: nn, walked: 0, edges: 0, coarse: 0, phases: [0.0; 4] })
    }

    /// What it was built from (Deps::valid).
    pub(crate) fn built_from(&self) -> u128 { self.valid }
    /// The number of crates it was built for.
    pub(crate) fn crate_count(&self) -> usize { self.crates.rows().len() }
    /// Whether a graph can be built over it (its component ids leave room for the new ones).
    pub(crate) fn ids_left(&self) -> bool { self.comp_end < 1 << 30 }
    /// Whether the graph holds the same closures as `other` (MAKEPAD_RUN_DEPS_VERIFY: a graph built
    /// over a stored one against one built from scratch; component ids are not compared).
    pub(crate) fn same_closures(&self, other: &Deps) -> Result<(), String> {
        let (a, b) = (self.nodes.rows(), other.nodes.rows());
        if a.len() != b.len() { return Err(format!("{} node rows, {} from scratch", a.len(), b.len())); }
        if let Some(r) = a.iter().zip(b).position(|(x, y)| (x.file, x.item, x.closure) != (y.file, y.item, y.closure)) {
            return Err(format!("node row {r} (file {} item {}) differs", a[r].file, a[r].item));
        }
        let (a, b) = (self.names.rows(), other.names.rows());
        if a.len() != b.len() { return Err(format!("{} name rows, {} from scratch", a.len(), b.len())); }
        if let Some(r) = a.iter().zip(b).position(|(x, y)| (x.name, x.krate, x.closure) != (y.name, y.krate, y.closure)) {
            return Err(format!("name row {r} (crate {}) differs", a[r].krate));
        }
        if self.macros.rows() != other.macros.rows() { return Err("macro names differ".into()); }
        let (a, b) = (self.crates.rows(), other.crates.rows());
        if a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x.always != y.always) { return Err("always closures differ".into()); }
        Ok(())
    }

    /// What a stored graph must have been built from to stand for this program's: the cache
    /// configuration (the Relax build), every crate's fingerprint, and the file numbering (the
    /// tables name items by file and item index).
    pub(crate) fn valid(prog: &Program, cfg: u128, fps: &[u128]) -> u128 {
        let mut h = Sip128::new();
        h.str("deps-valid");
        h.u32(DEPS_VERSION);
        h.u128(cfg);
        h.u32(fps.len() as u32);
        for f in fps { h.u128(*f); }
        h.u32(prog.files.len() as u32);
        h.u32(prog.boot_files as u32);
        for (i, p) in prog.file_paths.iter().enumerate().take(prog.boot_files.min(prog.files.len())) {
            h.str(p);
            h.u32(prog.file_crate.get(i).copied().unwrap_or(u32::MAX));
            h.u32(prog.files.get(i).map_or(0, |f| f.ast.items.len() as u32));
        }
        h.finish128()
    }

    /// The closures of the always nodes crate `c` can see.
    pub(crate) fn always(&self, c: u32) -> u128 { self.crates.rows().get(c as usize).map_or(0, |r| u2(r.always)) }

    /// The closure hash of definition `d`'s item: None for a sysroot crate's definitions (its
    /// interface fingerprint is in every key) and for a definition with no node (an item a fn body
    /// defines: the caller keys it coarsely).
    pub(crate) fn def_closure(&self, prog: &Program, d: DefId) -> Option<u128> {
        let mut d = d;
        for _ in 0..64 {
            let def = prog.def(d);
            if self.atomic.get(def.krate as usize).copied().unwrap_or(true) { return None; }
            let rows = self.nodes.rows();
            if let Ok(k) = rows.binary_search_by_key(&(def.file, def.item.0), |r| (r.file, r.item)) { return Some(u2(rows[k].closure)); }
            // a field, variant, closure, anonymous const or opaque type: its item's
            if !matches!(def.kind, DefKind::Field | DefKind::Variant | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Closure) || def.parent == NO_DEF {
                return None;
            }
            d = def.parent;
        }
        None
    }

    /// Whether `d`'s crate is in the graph.
    pub(crate) fn in_graph(&self, prog: &Program, d: DefId) -> bool {
        !self.atomic.get(prog.def(d).krate as usize).copied().unwrap_or(true)
    }

    /// The closure of what name `n` finds from crate `c`: the closures of the nodes found under it
    /// (0: none).
    pub(crate) fn name_closure(&self, c: u32, n: Name) -> u128 {
        let Some(uni) = self.universe.get(c as usize) else { return 0 };
        let rows = self.names.rows();
        let at = rows.partition_point(|r| r.name < n);
        let mut v: Vec<u128> = rows[at..].iter().take_while(|r| r.name == n).filter(|r| bit(uni, r.krate)).map(|r| u2(r.closure)).collect();
        if v.is_empty() { return 0; }
        v.sort_unstable();
        v.dedup();
        let mut h = Sip128::new();
        h.str("dep-name");
        for x in v { h.u128(x); }
        h.finish128()
    }

    /// Whether invocations of macro `m` (a `scan` invocation entry) can hide names.
    pub(crate) fn loud(&self, m: Name) -> bool {
        m & QUIET_BIT == 0 || self.macros.rows().binary_search(&(m & !QUIET_BIT)).is_ok()
    }
}

/// The names of the expansion text produced by macro invocations inside `[lo, hi)` (a fn body),
/// followed back to their call sites. None when the file's lazily parsed bodies are not parsed
/// (their expansions are not known).
pub(crate) fn exp_names(pf: &ParsedFile, lo: u32, hi: u32, lazy: bool) -> Option<Vec<Name>> {
    if lazy && pf.part.get().is_none() { return None; }
    let mut names = Vec::new();
    let regions = file_regions(pf);
    for (a, b) in region_closure(&regions, &[(lo, hi)]) {
        let text = pf.bytes(a, b);
        scan(text, &[(0, text.len() as u32)], &[], &mut names, &mut Vec::new());
    }
    names.sort_unstable();
    names.dedup();
    Some(names)
}

/// A file's expansion regions (its own text's and its parsed parts'): (direct call site, start,
/// end), sorted by call site. A call site inside another expansion's text is that position.
fn file_regions(pf: &ParsedFile) -> Vec<(u32, u32, u32)> {
    let mut v: Vec<(u32, u32, u32)> = pf.exp_map.iter().map(|(a, b, c)| (c, a, b)).collect();
    for p in pf.parts() { v.extend(p.exp_map.iter().map(|(a, b, c)| (c, a, b))); }
    v.sort_unstable();
    v
}

/// The regions (start, end) of the expansions called from within `seeds`, and of those called from
/// within them, transitively.
fn region_closure(regions: &[(u32, u32, u32)], seeds: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    let mut todo: Vec<(u32, u32)> = seeds.to_vec();
    let mut seen = std::collections::HashSet::new();
    let mut guard = 0usize;
    while let Some((a, b)) = todo.pop() {
        guard += 1;
        if guard > 1 << 20 { break; }
        let k = regions.partition_point(|r| r.0 < a);
        for &(c, x, y) in &regions[k..] {
            if c >= b { break; }
            if seen.insert((x, y)) {
                out.push((x, y));
                todo.push((x, y));
            }
        }
    }
    out
}

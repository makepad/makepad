//! Item-level demand for Rust file selection. Collection resolves every loaded
//! item's declaration (ADT fields, impl headers and bounds, constants, trait
//! signatures), so a reached file contributes those names as declarations.
//! A function, method or macro adds its references only once it is used: by
//! name, through its impl's self type and trait, or (inherent methods and
//! trait members) through a member name used by reached code.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Fn, Decl, Value, Trait, Impl, Use, Macro, Mod, Opaque }

#[derive(Clone)]
pub struct Member {
    pub name: String,
    pub range: (usize, usize),
    pub sig: (usize, usize),
    pub is_fn: bool,
    pub has_body: bool,
    pub nested: Vec<Nested>,
}

/// An item declared inside a function body. Collection (and `check`'s constant
/// evaluation) sees it whenever its file is loaded, used or not.
#[derive(Clone)]
pub struct Nested { pub range: (usize, usize), pub evaluated: bool }

#[derive(Clone)]
pub struct Head {
    /// Qualified self-type candidates; empty for a structural or parameter self type.
    pub selves: Vec<String>,
    /// Qualified trait candidates; empty for an inherent impl.
    pub traits: Vec<String>,
    pub inherent: bool,
}

#[derive(Clone)]
pub struct Item {
    pub module: String,
    pub names: Vec<String>,
    pub kind: Kind,
    pub range: (usize, usize),
    pub header: (usize, usize),
    pub members: Vec<Member>,
    pub head: Option<Head>,
    pub global: bool,
    /// A cold free function's signature is needed now; its body is resolved by
    /// the JIT on first call, using the ordinary deferred-module transaction.
    pub cold: bool,
    pub nested: Vec<Nested>,
}

fn word(src: &[u8], t: &Tok) -> String {
    String::from_utf8_lossy(&src[t.lo as usize..t.hi as usize]).trim_start_matches("r#").to_owned()
}

/// rs_expand's outline_item_end over a token slice: an item ends at a `;`, or
/// after the brace group that closes it (unless an initializer or alias is open).
fn item_end(toks: &[Tok], mut at: usize, hi: usize) -> usize {
    let mut semicolon = false;
    let mut angles = 0i32;
    while at < hi {
        match toks[at].kind {
            T::Lt => angles += 1,
            T::Gt => angles = (angles - 1).max(0),
            T::Shr => angles = (angles - 2).max(0),
            T::KwUse | T::KwStatic | T::KwType => semicolon = true,
            T::Eq if angles == 0 => semicolon = true,
            T::KwFn => semicolon = false,
            T::Semi => return at + 1,
            T::CloseBrace | T::Eof => return at.max(1),
            T::OpenParen | T::OpenBracket => at = demand::close(toks, at),
            T::OpenBrace => {
                at = demand::close(toks, at);
                if !semicolon && angles == 0 { return at + 1; }
            }
            _ => {}
        }
        at += 1;
    }
    hi
}

const GLOBAL_ATTRS: &[&[u8]] = &[b"lang", b"no_mangle", b"export_name", b"link", b"link_section", b"used",
    b"global_allocator", b"alloc_error_handler", b"panic_handler"];

/// Split `toks[lo..hi]` (one module scope) into items; inline modules recurse.
pub fn split(src: &[u8], toks: &[Tok], lo: usize, hi: usize, module: &str, out: &mut Vec<Item>) {
    let mut at = lo;
    while at < hi {
        let kind0 = toks[at].kind;
        if matches!(kind0, T::Semi | T::Eof | T::CloseBrace) { at += 1; continue; }
        let start = at;
        let mut global = false;
        let mut cold = false;
        // Attributes (inner ones are inert at item level).
        while at < hi && toks[at].kind == T::Pound {
            let open = at + 1 + usize::from(toks.get(at + 1).is_some_and(|t| t.kind == T::Not));
            if !toks.get(open).is_some_and(|t| t.kind == T::OpenBracket) { break; }
            let end = demand::close(toks, open);
            cold |= end == open + 2 && &src[toks[open + 1].lo as usize..toks[open + 1].hi as usize] == b"cold";
            global |= toks[open + 1..end.min(hi)].iter().any(|t| GLOBAL_ATTRS.contains(&&src[t.lo as usize..t.hi as usize]));
            at = end + 1;
        }
        if at >= hi { break; }
        let head = at;
        let end = item_end(toks, head, hi).max(head + 1).min(hi);
        // Visibility and qualifiers.
        let mut k = head;
        while k < end {
            match toks[k].kind {
                T::KwPub => {
                    k += 1;
                    if toks.get(k).is_some_and(|t| t.kind == T::OpenParen) { k = demand::close(toks, k) + 1; }
                }
                T::KwUnsafe | T::KwAsync => k += 1,
                T::KwConst if toks.get(k + 1).is_some_and(|t| matches!(t.kind, T::KwFn | T::KwUnsafe | T::KwAsync | T::KwExtern)) => k += 1,
                T::KwExtern if toks.get(k + 1).is_some_and(|t| t.kind == T::Str) && toks.get(k + 2).is_some_and(|t| matches!(t.kind, T::KwFn | T::KwUnsafe)) => k += 2,
                T::KwExtern if toks.get(k + 1).is_some_and(|t| matches!(t.kind, T::KwFn | T::KwUnsafe)) => k += 1,
                T::Ident if matches!(&src[toks[k].lo as usize..toks[k].hi as usize], b"default" | b"auto")
                    && toks.get(k + 1).is_some_and(|t| matches!(t.kind, T::KwFn | T::KwImpl | T::KwTrait | T::KwUnsafe | T::KwType | T::KwConst | T::KwAsync)) => k += 1,
                _ => break,
            }
        }
        let name_after = |i: usize| toks.get(i).filter(|t| matches!(t.kind, T::Ident | T::RawIdent)).map(|t| format!("{module}::{}", word(src, t)));
        let mut item = Item { module: module.to_owned(), names: Vec::new(), kind: Kind::Opaque, range: (start, end),
            header: (start, end), members: Vec::new(), head: None, global, cold, nested: Vec::new() };
        let open_brace = |from: usize| (from..end).find(|&i| toks[i].kind == T::OpenBrace);
        match toks.get(k).map(|t| t.kind) {
            Some(T::KwFn) => {
                item.kind = Kind::Fn;
                item.names.extend(name_after(k + 1));
                if let Some(body) = (k..end).find(|&i| toks[i].kind == T::OpenBrace && demand::close(toks, i) + 1 == end) {
                    item.header = (start, body);
                }
            }
            Some(T::KwStruct | T::KwEnum | T::KwType | T::KwStatic | T::KwConst) => {
                item.kind = if matches!(toks[k].kind, T::KwStatic | T::KwConst) { Kind::Value } else { Kind::Decl };
                // A static's initializer runs when code using the static is built, not at
                // load or signature collection: its header is the type part only, and the
                // initializer's names are loaded on demand (the post-boot loader).
                if toks[k].kind == T::KwStatic {
                    let mut depth = 0i32;
                    if let Some(eq) = (k..end).find(|&i| { match toks[i].kind { T::Lt | T::OpenParen | T::OpenBracket => depth += 1, T::Gt | T::CloseParen | T::CloseBracket => depth -= 1, T::Shr => depth -= 2, _ => {} } depth <= 0 && toks[i].kind == T::Eq }) {
                        item.header = (start, eq);
                    }
                }
                let mut n = k + 1;
                if toks.get(n).is_some_and(|t| t.kind == T::KwMut) { n += 1; }
                item.names.extend(name_after(n));
            }
            Some(T::Ident) if &src[toks[k].lo as usize..toks[k].hi as usize] == b"union" && toks.get(k + 1).is_some_and(|t| t.kind == T::Ident) => {
                item.kind = Kind::Decl;
                item.names.extend(name_after(k + 1));
            }
            Some(T::KwTrait) => {
                item.kind = Kind::Trait;
                item.names.extend(name_after(k + 1));
                if let Some(open) = open_brace(k) {
                    item.header = (start, open);
                    item.members = members(src, toks, open + 1, demand::close(toks, open), module);
                }
            }
            Some(T::KwImpl) => {
                item.kind = Kind::Impl;
                if let Some(open) = open_brace(k) {
                    item.header = (start, open);
                    item.members = members(src, toks, open + 1, demand::close(toks, open), module);
                }
            }
            Some(T::KwUse) => {
                item.kind = Kind::Use;
                let mut uses = Vec::new();
                let semi = (k..end).find(|&i| toks[i].kind == T::Semi).unwrap_or(end);
                super::super::use_tree(&|i| word(src, &toks[i]), toks, k + 1, semi, &[], module, false, &mut uses);
                item.names.extend(uses.into_iter().filter_map(|u| u.bind).filter(|b| b != "_").map(|b| format!("{module}::{b}")));
            }
            Some(T::KwExtern) if toks.get(k + 1).is_some_and(|t| t.kind == T::KwCrate) => {
                item.kind = Kind::Use;
                let alias = if toks.get(k + 3).is_some_and(|t| t.kind == T::KwAs) { k + 4 } else { k + 2 };
                item.names.extend(name_after(alias));
            }
            Some(T::KwExtern) => {
                // Foreign block: its declarations are resolved with the block.
                item.kind = Kind::Decl;
                if let Some(open) = open_brace(k) {
                    let close = demand::close(toks, open);
                    for i in open + 1..close {
                        if matches!(toks[i].kind, T::KwFn | T::KwStatic | T::KwType) { item.names.extend(name_after(i + 1 + usize::from(toks.get(i + 1).is_some_and(|t| t.kind == T::KwMut)))); }
                    }
                }
            }
            Some(T::KwMod) => {
                item.kind = Kind::Mod;
                item.names.extend(name_after(k + 1));
                if toks.get(k + 2).is_some_and(|t| t.kind == T::OpenBrace) {
                    let close = demand::close(toks, k + 2);
                    let inner = format!("{module}::{}", word(src, &toks[k + 1]));
                    out.push(item);
                    split(src, toks, k + 3, close, &inner, out);
                    at = end;
                    continue;
                }
            }
            Some(T::Ident) if &src[toks[k].lo as usize..toks[k].hi as usize] == b"macro_rules" && toks.get(k + 1).is_some_and(|t| t.kind == T::Not) => {
                item.kind = Kind::Macro;
                item.names.extend(name_after(k + 2));
            }
            _ => {}
        }
        if item.kind == Kind::Impl { item.head = Some(outline_head(src, toks, item.header)); }
        if item.kind == Kind::Fn { item.nested = nested_items(src, toks, item.range); }
        out.push(item);
        at = end;
    }
}

fn members(src: &[u8], toks: &[Tok], lo: usize, hi: usize, module: &str) -> Vec<Member> {
    let mut items = Vec::new();
    split(src, toks, lo, hi, module, &mut items);
    items.into_iter().map(|item| {
        let name = item.names.first().map(|n| n.rsplit("::").next().unwrap_or("").to_owned()).unwrap_or_default();
        let is_fn = item.kind == Kind::Fn;
        let body = if is_fn { (item.range.0..item.range.1).rev().find(|&i| toks[i].kind == T::CloseBrace)
            .map(|close| (item.range.0..close).find(|&i| toks[i].kind == T::OpenBrace && demand::close(toks, i) == close)).flatten() } else { None };
        Member { name, range: item.range, sig: (item.range.0, body.unwrap_or(item.range.1)), is_fn, has_body: body.is_some(), nested: item.nested }
    }).collect()
}

/// Items declared anywhere inside a function's body: statics and named
/// constants (evaluated by `check`), and type declarations (their fields are
/// resolved by collection).
fn nested_items(src: &[u8], toks: &[Tok], range: (usize, usize)) -> Vec<Nested> {
    let Some(open) = (range.0..range.1).find(|&i| toks[i].kind == T::OpenBrace && demand::close(toks, i) + 1 == range.1) else { return Vec::new(); };
    let mut out = Vec::new();
    let mut at = open + 1;
    while at + 1 < range.1 {
        let evaluated = match toks[at].kind {
            T::KwStatic => Some(true),
            T::KwConst if toks.get(at + 2).is_some_and(|t| t.kind == T::Colon) && toks.get(at + 1).is_some_and(|t| matches!(t.kind, T::Ident | T::RawIdent)) => Some(true),
            T::KwStruct | T::KwEnum | T::KwType => Some(false),
            T::Ident if &src[toks[at].lo as usize..toks[at].hi as usize] == b"union" && toks.get(at + 1).is_some_and(|t| t.kind == T::Ident) => Some(false),
            _ => None,
        };
        if let Some(evaluated) = evaluated {
            let end = item_end(toks, at, range.1 - 1).min(range.1 - 1);
            out.push(Nested { range: (at, end), evaluated });
            at = end.max(at + 1);
            continue;
        }
        at += 1;
    }
    out
}

/// Split an impl header during outline expansion, before namespace preparation.
fn outline_head(src: &[u8], toks: &[Tok], header: (usize, usize)) -> Head {
    let (lo, hi) = header;
    let Some(mut at) = (lo..hi).find(|&i| toks[i].kind == T::KwImpl) else { return Head { selves: Vec::new(), traits: Vec::new(), inherent: true } };
    at += 1;
    let mut params = BTreeSet::new();
    if toks.get(at).is_some_and(|t| t.kind == T::Lt) {
        let mut depth = 0i32;
        let mut parameter = true;
        while at < hi {
            match toks[at].kind {
                T::Lt => depth += 1,
                T::Gt => depth -= 1,
                T::Shr => depth -= 2,
                T::Comma if depth == 1 => parameter = true,
                T::Ident | T::RawIdent if depth == 1 && parameter => { params.insert(word(src, &toks[at])); parameter = false; }
                T::Lifetime | T::KwConst if depth == 1 => parameter = false,
                _ => {}
            }
            at += 1;
            if depth <= 0 { break; }
        }
    }
    let end = (at..hi).find(|&i| toks[i].kind == T::KwWhere).unwrap_or(hi);
    // A `for` at angle depth 0 separates the trait from the self type.
    let mut depth = 0i32;
    let mut split_at = None;
    for i in at..end {
        match toks[i].kind {
            T::Lt => depth += 1,
            T::Gt => depth -= 1,
            T::Shr => depth -= 2,
            T::KwFor if depth == 0 => { split_at = Some(i); break; }
            T::OpenParen | T::OpenBracket => {}
            _ => {}
        }
    }
    let path = |from: usize, to: usize| -> Option<Vec<String>> {
        let mut i = from;
        if toks.get(i).is_some_and(|t| t.kind == T::Not) { return None; }
        let mut segs = Vec::new();
        if toks.get(i).is_some_and(|t| t.kind == T::PathSep) { segs.push(String::new()); i += 1; }
        while i < to {
            match toks[i].kind {
                T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper | T::KwSelfType => segs.push(word(src, &toks[i])),
                _ => return None,
            }
            i += 1;
            if i < to && toks[i].kind == T::Lt {
                let mut depth = 0i32;
                while i < to {
                    match toks[i].kind { T::Lt => depth += 1, T::Gt => depth -= 1, T::Shr => depth -= 2, _ => {} }
                    i += 1;
                    if depth <= 0 { break; }
                }
            }
            if i < to && toks[i].kind == T::PathSep { i += 1; continue; }
            break;
        }
        (!segs.is_empty()).then_some(segs)
    };
    let (trait_range, self_range) = match split_at { Some(f) => (Some((at, f)), (f + 1, end)), None => (None, (at, end)) };
    let selves = path(self_range.0, self_range.1)
        .filter(|segs| !(segs.len() == 1 && params.contains(&segs[0])))
        .map(|segs| vec![segs.join("::")]).unwrap_or_default();
    let traits = trait_range.and_then(|(a, b)| path(a, b)).map(|segs| vec![segs.join("::")]).unwrap_or_default();
    Head { selves, traits, inherent: trait_range.is_none() }
}

/// Resolve a file's split impl heads once its crate's namespace is prepared.
pub(crate) fn qualify(names: &demand::Names, ci: usize, items: &mut [Item]) {
    for item in items {
        if let Some(head) = &mut item.head { qualify_head(names, ci, &item.module, head); }
    }
}

/// Resolve the already split paths once the namespace is complete.
fn qualify_head(names: &demand::Names, ci: usize, module: &str, head: &mut Head) {
    let qualify = |path: &str| -> Vec<String> {
        let segs: Vec<String> = path.split("::").map(str::to_owned).collect();
        let own = format!("{module}::{}", segs.join("::"));
        if segs.len() == 1 && names.is_type(&own) { return vec![own]; }
        names.resolve_path(ci, module, &segs).into_iter().filter(|n| !n.contains("::prelude::") || names.is_type(n) || names.is_trait(n)).collect()
    };
    head.selves = head.selves.iter().flat_map(|path| qualify(path)).collect();
    head.traits = head.traits.iter().flat_map(|path| qualify(path)).collect();
}

/// Names reached code can call as members: an identifier after `.` (method
/// call or field), or the last segment of a path that is called (`T::f(`,
/// `T::f::<..>(`). Module segments of a path are not members.
pub fn member_names(src: &[u8], toks: &[Tok], out: &mut BTreeSet<String>) {
    for (i, t) in toks.iter().enumerate() {
        // `dyn Trait`: a vtable needs every method of each implementation.
        if t.kind == T::KwDyn {
            let mut at = i + 1;
            let mut last = None;
            while at < toks.len() && matches!(toks[at].kind, T::Ident | T::RawIdent | T::PathSep | T::KwCrate | T::KwSuper | T::KwSelfValue) {
                if matches!(toks[at].kind, T::Ident | T::RawIdent) { last = Some(word(src, &toks[at])); }
                at += 1;
            }
            if let Some(last) = last { out.insert(format!("dyn {last}")); }
            continue;
        }
        if !matches!(t.kind, T::Ident | T::RawIdent) || i == 0 { continue; }
        let member = match toks[i - 1].kind {
            T::Dot => true,
            T::PathSep => match toks.get(i + 1).map(|t| t.kind) {
                Some(T::OpenParen) => true,
                Some(T::PathSep) => toks.get(i + 2).is_some_and(|t| t.kind == T::Lt),
                _ => false,
            },
            _ => false,
        };
        if member {
            let name = word(src, t);
            if !out.contains(&name) { out.insert(name); }
        }
    }
}

#[derive(Clone)]
pub struct FileInput<'a> {
    pub ci: usize,
    pub path: &'a str,
    pub module: &'a str,
    pub index: &'a FileIndex,
    pub surface: Option<(&'a [u8], &'a [Tok])>,
    pub eager_bodies: Option<&'a BTreeSet<u32>>,
    pub items: Arc<Vec<Item>>,
    /// Every item is used once the file is reached (unindexed body impls,
    /// unexpanded item macros, an opaque module scope).
    pub force: bool,
    pub root: bool,
}

impl FileInput<'_> {
    fn tokens(&self) -> Option<(&[u8], &[Tok])> {
        self.surface
    }
    fn eager(&self) -> Option<&BTreeSet<u32>> {
        self.eager_bodies
    }
}

/// Equality of the planner's token transfer, ignoring literal values and byte
/// offsets. Reference lookup and member discovery observe literal *kinds*, not
/// their values. Keep eager-body markers relative to tokens, not source bytes.
pub(super) fn same_transfer(a: (&[u8], &[Tok], Option<&BTreeSet<u32>>), b: (&[u8], &[Tok], Option<&BTreeSet<u32>>)) -> bool {
    a.1.len() == b.1.len() && a.1.iter().zip(b.1).all(|(at, bt)| {
        let (alo, blo) = (at.lo, bt.lo);
        at.kind == bt.kind
            && (at.kind != T::OpenBrace || a.2.is_some_and(|s| s.contains(&alo)) == b.2.is_some_and(|s| s.contains(&blo)))
            && (matches!(at.kind, T::Str | T::RawStr | T::Int | T::Float | T::Char | T::Byte)
                || a.0[at.lo as usize..at.hi as usize] == b.0[bt.lo as usize..bt.hi as usize])
    })
}

pub struct Closure {
    pub reached: Vec<bool>,
    pub reasons: Vec<(String, String)>,
    pub items: usize,
    pub used_items: usize,
    /// MAKEPAD_RUN_ITEMS_TRACE only: per reached file, the file whose evaluated
    /// reference first produced the name/member that reached it, and that key.
    pub parents: Vec<Option<(Option<usize>, String)>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Strength { Declared, Used }

/// A crate's boundary names while its closure runs: each evaluation's references are folded in as they
/// are produced (a name repeats across thousands of evaluations; the sets keep it once).
#[derive(Default)]
struct Accumulated { declared: BTreeSet<String>, used: BTreeSet<String>, members: BTreeSet<String> }

struct Work { file: usize, module: String, ranges: Vec<(usize, usize)>, strength: Strength, exclude: Vec<String> }

pub fn closure(names: &mut demand::Names, files: &mut [FileInput<'_>], seeds: BTreeSet<String>, implicit: &BTreeSet<String>, jobs: usize,
    profile: bool, parts: &mut [DemandPart], incoming: Option<&part::Boundary>,
) -> Closure {
    let entry = std::time::Instant::now();
    for file in files.iter() {
        parts[file.ci].boundary = part::Boundary::default();
    }
    // per crate (parts index), merged from the workers as each finishes
    let accumulated: Vec<relax_session::sync::Mutex<Accumulated>> = (0..parts.len()).map(|_| Default::default()).collect();
    let n = files.len();
    // Splits/raw heads were built by outline workers and qualified as each
    // crate's namespace was prepared; each item list is moved, never cloned.
    let parsed: Vec<Arc<Vec<Item>>> = files.iter_mut().map(|f| std::mem::take(&mut f.items)).collect();
    let mut trait_members: HashMap<String, BTreeSet<String>> = incoming.map(|b| b.traits.iter().map(|(n, m)| (n.clone(), m.clone())).collect()).unwrap_or_default();
    for (f, items) in parsed.iter().enumerate() {
        for item in items.iter() {
            if item.kind == Kind::Trait {
                if let Some(name) = item.names.first() {
                    trait_members.entry(name.clone()).or_default().extend(item.members.iter().map(|m| m.name.clone()));
                    parts[files[f].ci].boundary.traits.entry(name.clone()).or_default().extend(item.members.iter().map(|m| m.name.clone()));
                }
            }
        }
    }
    // Reverse dependency indexes. A new name schedules only the files/items
    // it can affect, including impl conjunctions and dynamic dispatch markers.
    let mut declarations: HashMap<String, Vec<usize>> = HashMap::default();
    // Impl headers are globally visible through the self-type/trait index,
    // even when no body reaches the module containing the impl. Concrete
    // candidates are keyed by self type; blanket/structural ones by trait.
    // Loading their declarations does not demand their method bodies.
    let mut impl_files: HashMap<String, Vec<usize>> = HashMap::default();
    let mut modules: HashMap<String, Vec<usize>> = HashMap::default();
    let mut users: HashMap<String, Vec<(usize, usize)>> = HashMap::default();
    let mut member_users: HashMap<String, Vec<(usize, usize)>> = HashMap::default();
    let mut queue = BTreeSet::new();
    let mut reach_queue = BTreeMap::new();
    for (f, items) in parsed.iter().enumerate() {
        modules.entry(files[f].module.to_owned()).or_default().push(f);
        declarations.entry(files[f].module.to_owned()).or_default().push(f);
        for name in files[f].index.impls.iter().map(|(_, ty)| ty).chain(&files[f].index.blankets) {
            impl_files.entry(name.clone()).or_default().push(f);
        }
        if files[f].root || files[f].force || files[f].surface.is_none() {
            let cause = if files[f].root { "crate-root" } else if files[f].force { "forced-file" } else { "no-surface" };
            reach_queue.insert(f, (cause.to_owned(), String::new()));
        }
        for (i, item) in items.iter().enumerate() {
            for name in &item.names {
                declarations.entry(name.clone()).or_default().push(f);
                users.entry(name.clone()).or_default().push((f, i));
            }
            for member in &item.members { member_users.entry(member.name.clone()).or_default().push((f, i)); }
            if files[f].force || item.global || item.head.is_some() { queue.insert((f, i)); }
            if let Some(head) = &item.head {
                for name in head.selves.iter().chain(&head.traits) { users.entry(name.clone()).or_default().push((f, i)); }
                for name in &head.traits {
                    member_users.entry(format!("dyn {}", name.rsplit("::").next().unwrap_or(name))).or_default().push((f, i));
                    if let Some(members) = trait_members.get(name.as_str()) {
                        for member in members { member_users.entry(member.clone()).or_default().push((f, i)); }
                    }
                }
            }
        }
    }
    for files in declarations.values_mut().chain(impl_files.values_mut()) { files.sort_unstable(); files.dedup(); }
    for items in users.values_mut().chain(member_users.values_mut()) { items.sort_unstable(); items.dedup(); }
    let mut used: HashSet<String> = HashSet::default();
    let mut declared: HashSet<String> = HashSet::default();
    let mut members: HashSet<String> = HashSet::default();
    let mut reached = vec![false; n];
    let mut reasons = vec![(String::new(), String::new()); n];
    let mut item_used: Vec<Vec<bool>> = parsed.iter().map(|items| vec![false; items.len()]).collect();
    let mut member_used: Vec<Vec<Vec<bool>>> = parsed.iter().map(|items| items.iter().map(|i| vec![false; i.members.len()]).collect()).collect();
    let trace = std::env::var_os("MAKEPAD_RUN_ITEMS_TRACE").is_some();
    let started = std::time::Instant::now();
    if trace { eprintln!("rs-items-prep: {:?}", entry.elapsed()); }
    let mut causes = BTreeMap::<String, usize>::new();
    // Provenance (trace only): name/member -> first file whose reference produced it.
    let mut origin: HashMap<String, usize> = HashMap::default();
    let mut member_origin: HashMap<String, usize> = HashMap::default();
    let mut trigger: Vec<Option<String>> = vec![None; n];
    let mut pending: Vec<_> = seeds.into_iter().map(|name| (Strength::Used, name)).collect();
    let mut pending_members: BTreeSet<String> = BTreeSet::new();
    if let Some(incoming) = incoming {
        pending.extend(incoming.declared.iter().cloned().map(|n| (Strength::Declared, n)));
        pending.extend(incoming.used.iter().cloned().map(|n| (Strength::Used, n)));
        pending_members.extend(incoming.members.iter().cloned());
    }
    let mut visits = 0usize;
    let mut spent = [std::time::Duration::ZERO; 4];
    loop {
        let mut work: Vec<Work> = Vec::new();
        let t0 = std::time::Instant::now();
        for (strength, name) in pending.drain(..) {
            if declared.insert(name.clone()) {
                for &f in declarations.get(name.as_str()).into_iter().flatten() {
                    if !reached[f] { reach_queue.entry(f).or_insert_with(|| { if trace { trigger[f] = Some(name.clone()); } ("name".to_owned(), name.clone()) }); }
                }
                for &f in impl_files.get(name.as_str()).into_iter().flatten() {
                    if !reached[f] { reach_queue.entry(f).or_insert_with(|| { if trace { trigger[f] = Some(name.clone()); } ("impl-index".to_owned(), name.clone()) }); }
                }
                // Forwarding scopes matter even when a glob has no local item.
                if let Some((scope, _)) = name.rsplit_once("::") {
                    for &f in modules.get(scope).into_iter().flatten() {
                        if !reached[f] { reach_queue.entry(f).or_insert_with(|| { if trace { trigger[f] = Some(name.clone()); } ("scope".to_owned(), scope.to_owned()) }); }
                    }
                }
            }
            if strength == Strength::Used && used.insert(name.clone()) {
                queue.extend(users.get(name.as_str()).into_iter().flatten().copied());
            }
        }
        for name in std::mem::take(&mut pending_members) {
            if members.insert(name.clone()) { queue.extend(member_users.get(&name).into_iter().flatten().copied()); }
        }
        let reach = |f: usize, why: (&str, &str), reached: &mut Vec<bool>, reasons: &mut Vec<(String, String)>, work: &mut Vec<Work>, queue: &mut BTreeSet<(usize, usize)>| {
            if reached[f] { return; }
            reached[f] = true;
            reasons[f] = (why.0.to_owned(), why.1.to_owned());
            work.push(Work { file: f, module: files[f].module.to_owned(), ranges: Vec::new(), strength: Strength::Declared, exclude: vec![files[f].module.to_owned()] });
            // The loader resolves every loaded item's declaration.
            for (i, item) in parsed[f].iter().enumerate() {
                if item.kind == Kind::Opaque { queue.insert((f, i)); }
                let mut declarations = Vec::new();
                let mut uses = Vec::new();
                match item.kind {
                    Kind::Decl => declarations.push(item.range),
                    Kind::Value if item.header != item.range => declarations.push(item.header),
                    Kind::Value => uses.push(item.range),
                    Kind::Trait => {
                        declarations.push(item.header);
                        declarations.extend(item.members.iter().map(|m| m.sig));
                    }
                    Kind::Impl => {
                        declarations.push(item.header);
                        declarations.extend(item.members.iter().filter(|m| !m.is_fn).map(|m| m.range));
                    }
                    // Trait imports, glob edges (their declaring scope) and
                    // unexpanded item macros are observable without a lookup.
                    Kind::Use | Kind::Opaque => uses.push(item.range),
                    Kind::Fn | Kind::Mod | Kind::Macro => {}
                }
                let eager = |range: (usize, usize)| files[f].tokens().is_some_and(|(_, toks)|
                    files[f].eager().is_some_and(|bodies| toks[range.0..range.1].iter()
                        .any(|t| { let lo = t.lo; t.kind == T::OpenBrace && bodies.contains(&lo) })));
                if item.kind == Kind::Fn && eager(item.range) { uses.push(item.range); }
                uses.extend(item.members.iter().filter(|m| m.is_fn && eager(m.range)).map(|m| m.range));
                for nested in item.nested.iter().chain(item.members.iter().flat_map(|m| &m.nested)) {
                    if nested.evaluated { uses.push(nested.range); } else { declarations.push(nested.range); }
                }
                if !declarations.is_empty() { work.push(Work { file: f, module: item.module.clone(), ranges: declarations, strength: Strength::Declared, exclude: item.names.clone() }); }
                if !uses.is_empty() { work.push(Work { file: f, module: item.module.clone(), ranges: uses, strength: Strength::Used, exclude: Vec::new() }); }
            }
        };
        let t1 = std::time::Instant::now();
        spent[0] += t1 - t0;
        for (f, why) in std::mem::take(&mut reach_queue) {
            reach(f, (&why.0, &why.1), &mut reached, &mut reasons, &mut work, &mut queue);
        }
        let t2 = std::time::Instant::now();
        spent[1] += t2 - t1;
        while let Some((f, i)) = queue.pop_first() {
            visits += 1;
            let item = &parsed[f][i];
            if !item_used[f][i] {
                let cause = if files[f].force { Some("forced") } else if item.global { Some("global") }
                    else if item.kind == Kind::Opaque && reached[f] { Some("opaque") }
                    else if item.names.iter().any(|n| used.contains(n.as_str())) { Some("name") }
                    else if let Some(h) = &item.head {
                        let selected = if h.selves.is_empty() { true } else { h.selves.iter().any(|s| used.contains(s.as_str())) };
                        if !selected { None }
                        else if h.inherent { Some(if h.selves.is_empty() { "impl-inherent-structural" } else { "impl-inherent" }) }
                        else if h.traits.is_empty() { Some("impl-unresolved-trait") }
                        else if h.traits.iter().any(|t| used.contains(t.as_str())) { Some(if h.selves.is_empty() { "impl-unkeyed-trait-used" } else { "impl-trait-used" }) }
                        // An absent member index is not evidence of a call.
                        // In-scope/implicit traits are demanded by name above;
                        // otherwise only a known called member selects an impl.
                        else if h.traits.iter().any(|t| trait_members.get(t.as_str()).is_some_and(|ms| ms.iter().any(|m| members.contains(m.as_str())))) { Some("impl-member") }
                        else { None }
                    } else { None };
                if let Some(cause) = cause.filter(|_| trace) { *causes.entry(format!("{cause} {:?}", item.kind)).or_insert(0) += 1; }
                if cause.is_some() {
                    item_used[f][i] = true;
                    if !reached[f] {
                        if trace {
                            trigger[f] = match cause {
                                Some("name") => item.names.iter().find(|n| used.contains(n.as_str())).cloned(),
                                _ => item.head.as_ref().and_then(|h| h.selves.iter().find(|s| used.contains(s.as_str()))
                                    .or_else(|| h.traits.iter().find(|t| used.contains(t.as_str()))).cloned()
                                    .or_else(|| h.traits.iter().flat_map(|t| trait_members.get(t.as_str()).into_iter().flatten())
                                        .find(|m| members.contains(m.as_str())).map(|m| format!(".{m}")))),
                            };
                        }
                        let why = match &item.head {
                            Some(h) => format!("impl {:?} for {:?}{}", h.traits, h.selves,
                                if h.inherent { " inherent" } else if h.traits.iter().any(|t| used.contains(t.as_str())) { " trait-used" } else { " member-or-unknown" }),
                            None => format!("{:?} {:?}", item.kind, item.names.first()),
                        };
                        reach(f, ("item", &why), &mut reached, &mut reasons, &mut work, &mut queue);
                    }
                    let ranges = match item.kind {
                        Kind::Mod => Vec::new(),
                        Kind::Trait => std::iter::once(item.header).chain(item.members.iter().map(|m| m.sig)).collect(),
                        Kind::Impl => std::iter::once(item.header).chain(item.members.iter().filter(|m| !m.is_fn).map(|m| m.range)).collect(),
                        Kind::Value => vec![item.header],
                        Kind::Fn if item.cold && !files[f].force => vec![item.header],
                        _ => vec![item.range],
                    };
                    if !ranges.is_empty() { work.push(Work { file: f, module: item.module.clone(), ranges, strength: Strength::Used, exclude: Vec::new() }); }
                }
            }
            if item_used[f][i] && matches!(item.kind, Kind::Impl | Kind::Trait) {
                // A trait implementation's methods are called by member name,
                // except through syntax/builtin expansion (the implicit std
                // traits) or a vtable (`dyn Trait` anywhere in used code).
                let whole_impl = item.kind == Kind::Impl && item.head.as_ref().is_some_and(|h| !h.inherent
                    && (h.traits.is_empty() || h.traits.iter().any(|t| implicit.contains(t.as_str())
                        || members.contains(&format!("dyn {}", t.rsplit("::").next().unwrap_or(t))))));
                let mut ranges = Vec::new();
                for (m, member) in item.members.iter().enumerate() {
                    if member_used[f][i][m] || !member.is_fn { continue; }
                    let on = files[f].force || whole_impl
                        || (item.kind == Kind::Trait && !member.has_body) || members.contains(&member.name);
                    if on {
                        if trace { *causes.entry(if whole_impl { "member-of-trait-impl".into() } else if files[f].force { "member-forced".into() } else { format!("member-by-name {:?}", item.kind) }).or_insert(0) += 1; }
                        member_used[f][i][m] = true;
                        if item.kind == Kind::Impl || member.has_body { ranges.push(member.range); }
                    }
                }
                if !ranges.is_empty() { work.push(Work { file: f, module: item.module.clone(), ranges, strength: Strength::Used, exclude: Vec::new() }); }
            }
        }
        spent[2] += t2.elapsed();
        if trace { eprintln!("rs-items-round: spent={spent:?} work={} ranges={} visits={visits} at={:?}", work.len(), work.iter().map(|w| w.ranges.len()).sum::<usize>(), started.elapsed()); }
        if work.is_empty() {
            if reach_queue.is_empty() && queue.is_empty() { break; }
            continue;
        }
        work.sort_by(|a, b| (files[a.file].ci, &a.module).cmp(&(files[b.file].ci, &b.module)));
        // Reaching a declaration and then using it often schedules exactly the
        // same ranges twice. Evaluate that source once, retaining every original
        // contribution (including its strength, exclusions and position).
        let mut unique = HashMap::<(usize, &str, &[(usize, usize)]), usize>::default();
        let mut evaluations = Vec::<Vec<usize>>::new();
        let mut costs = Vec::<usize>::new();
        let mut input_ranges = 0usize;
        let mut input_tokens = 0usize;
        for (at, w) in work.iter().enumerate() {
            let cost = files[w.file].tokens().map_or(0, |(_, toks)| w.ranges.iter()
                .map(|&(lo, hi)| hi.min(toks.len()) - lo.min(toks.len())).sum::<usize>());
            input_ranges += w.ranges.len();
            input_tokens += cost;
            let key = (w.file, w.module.as_str(), w.ranges.as_slice());
            if let Some(&evaluation) = unique.get(&key) {
                evaluations[evaluation].push(at);
            } else {
                unique.insert(key, evaluations.len());
                evaluations.push(vec![at]);
                costs.push(cost);
            }
        }
        drop(unique);
        let tokens = costs.iter().sum::<usize>();
        let ranges = evaluations.iter().map(|group| work[group[0]].ranges.len()).sum::<usize>();
        // Fixed, scope-local batches make lookup reuse independent of which
        // worker wins the next claim. Token weight splits large scopes; small
        // scopes remain together. This is scheduling granularity, not a limit.
        let target = tokens.div_ceil(jobs.max(1)).max(1);
        let mut batches = Vec::new();
        let mut begin = 0;
        let mut cost = 0;
        for (at, group) in evaluations.iter().enumerate() {
            let first = &work[evaluations[begin][0]];
            let w = &work[group[0]];
            if at > begin && (cost >= target || files[first.file].ci != files[w.file].ci || first.module != w.module) {
                batches.push((begin..at, cost));
                begin = at;
                cost = 0;
            }
            cost += costs[at];
        }
        batches.push((begin..evaluations.len(), cost));
        // Start heavier batches first so a large scope does not become the tail.
        // Each batch still evaluates its ranges in their original order.
        batches.sort_by_key(|(batch, cost)| (std::cmp::Reverse(*cost), batch.start));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let eval_started = std::time::Instant::now();
        // The demand sets are read-only while workers evaluate: each worker
        // also selects the names that are new to them, so the serial merge
        // only moves sets and queues those names.
        let (seen_declared, seen_used, seen_members) = (&declared, &used, &members);
        let accumulated = &accumulated;
        let (files, names) = (&*files, &*names);
        let evaluation = crate::load_profile::span(profile, "rust-demand-reference-eval", "load");
        let evaluated = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..jobs.max(1).min(batches.len())).map(|_| {
                let (work, next, batches, evaluations) = (&work, &next, &batches, &evaluations);
                scope.spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                    let _mem = crate::memstat::tag(crate::memstat::Tag::Planner);
                    let cpu = std::time::Instant::now();
                    let mut out = Vec::new();
                    let mut local: HashMap<usize, Accumulated> = HashMap::default();
                    let mut counts = [0usize; 4];
                    let _profile = crate::load_profile::span(profile, "rust-demand-reference-worker", "load");
                    loop {
                        let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((batch, _)) = batches.get(at) else { break; };
                        let mut lookups = demand::Lookups::default();
                        for group in &evaluations[batch.clone()] {
                            let w = &work[group[0]];
                            let needs_members = group.iter().any(|&at| work[at].strength == Strength::Used);
                            let mut refs = BTreeSet::new();
                            let mut found = BTreeSet::new();
                            if let Some((src, toks)) = files[w.file].tokens() {
                                for &(lo, hi) in &w.ranges {
                                    let slice = &toks[lo.min(toks.len())..hi.min(toks.len())];
                                    refs.extend(names.references_shared(files[w.file].ci, &w.module, src, slice, &mut lookups));
                                    if needs_members { member_names(src, slice, &mut found); }
                                }
                            }
                            if w.ranges.is_empty() { refs.insert(w.module.clone()); }
                            for (index, &at) in group.iter().enumerate() {
                                let w = &work[at];
                                let last = index + 1 == group.len();
                                let mut refs = if last { std::mem::take(&mut refs) } else { refs.clone() };
                                let found = if w.strength != Strength::Used { BTreeSet::new() }
                                    else if last { std::mem::take(&mut found) } else { found.clone() };
                                for name in &w.exclude {
                                    // A loaded type/trait needs its indexed impl
                                    // declarations even without a body-use edge.
                                    // Record this in the crate boundary as well.
                                    if w.strength == Strength::Declared && names.is_type(name) { refs.insert(name.clone()); }
                                    else if !w.ranges.is_empty() { refs.remove(name); }
                                }
                                let fresh: Vec<_> = refs.iter().filter(|name| !seen_declared.contains(name.as_str())
                                    || w.strength == Strength::Used && !seen_used.contains(name.as_str())).map(|name| (w.strength, name.clone())).collect();
                                let fresh_members: Vec<_> = found.iter().filter(|name| !seen_members.contains(name.as_str())).cloned().collect();
                                let used = w.strength == Strength::Used;
                                let acc = local.entry(files[w.file].ci).or_default();
                                if used { acc.used.extend(refs); } else { acc.declared.extend(refs); }
                                acc.members.extend(found);
                                out.push((at, (w.file, fresh, fresh_members)));
                            }
                        }
                        for (total, count) in counts.iter_mut().zip([lookups.hits, lookups.misses, lookups.glob_queries, lookups.glob_bindings]) { *total += count; }
                    }
                    for (ci, acc) in local {
                        let mut total = accumulated[ci].lock().unwrap();
                        total.declared.extend(acc.declared); total.used.extend(acc.used); total.members.extend(acc.members);
                    }
                    if trace { eprintln!("rs-items-eval: worker_us={} hits={} misses={}", cpu.elapsed().as_micros(), counts[0], counts[1]); }
                    (out, counts)
                })
            }).collect();
            handles.into_iter().map(|h| h.join().expect("rs item reference worker")).collect::<Vec<_>>()
        });
        let mut results = Vec::with_capacity(work.len());
        let mut counts = [0usize; 4];
        for (out, worker_counts) in evaluated {
            results.extend(out);
            for (total, count) in counts.iter_mut().zip(worker_counts) { *total += count; }
        }
        // Dynamic work claiming changes neither contribution order, first-demand
        // provenance nor publication order. Lookup counts are deterministic too.
        results.sort_by_key(|(at, _)| *at);
        drop(evaluation);
        if profile {
            eprintln!("rust-demand-reference-work: work={} evaluations={} batches={} input_ranges={input_ranges} ranges={ranges} input_tokens={input_tokens} tokens={tokens} hits={} misses={} glob_queries={} glob_bindings={} visits={visits}",
                work.len(), evaluations.len(), batches.len(), counts[0], counts[1], counts[2], counts[3]);
        }
        if trace { eprintln!("rs-items-eval-round: wall_us={}", eval_started.elapsed().as_micros()); }
        let t3 = std::time::Instant::now();
        for (_, (f, fresh, found)) in results {
            if trace {
                for (_, name) in &fresh { origin.entry(name.clone()).or_insert(f); }
                for name in &found { member_origin.entry(name.clone()).or_insert(f); }
            }
            pending.extend(fresh);
            pending_members.extend(found);
        }
        spent[3] += t3.elapsed();
    }
    if trace {
        for (cause, count) in &causes { eprintln!("rs-items-cause: {count} {cause}"); }
        let declaration_only = (0..n).filter(|&f| reached[f] && !item_used[f].iter().any(|u| *u)).count();
        eprintln!("rs-items: reached={} declaration-only={declaration_only}", reached.iter().filter(|r| **r).count());
        for f in (0..n).filter(|&f| reached[f] && !item_used[f].iter().any(|u| *u)) { eprintln!("rs-items-declaration-only: {:?} {:?}", files[f].module, reasons[f]); }
    }
    let items = parsed.iter().map(|items| items.len()).sum();
    let used_items = item_used.iter().map(|v| v.iter().filter(|u| **u).count()).sum();
    let parents = if trace {
        trigger.into_iter().map(|key| key.map(|key| {
            let from = match key.strip_prefix('.') { Some(m) => member_origin.get(m), None => origin.get(&key) };
            (from.copied(), key)
        })).collect()
    } else { Vec::new() };
    // Each crate's boundary: its folded names, a declaration only where nothing used it.
    for (ci, acc) in accumulated.into_iter().enumerate() {
        let Accumulated { mut declared, used, members } = acc.into_inner().unwrap();
        if declared.is_empty() && used.is_empty() && members.is_empty() { continue; }
        declared.retain(|name| !used.contains(name));
        let boundary = &mut parts[ci].boundary;
        boundary.declared = declared; boundary.used = used; boundary.members = members;
    }
    for file in files.iter() { parts[file.ci].key = 0; parts[file.ci].reached.clear(); parts[file.ci].counts = (0, 0); }
    for (f, (file, reached)) in files.iter().zip(&reached).enumerate() {
        parts[file.ci].counts.0 += parsed[f].len();
        parts[file.ci].counts.1 += item_used[f].iter().filter(|used| **used).count();
        if *reached { parts[file.ci].reached.insert(file.path.to_owned()); }
    }
    Closure { reached, reasons, items, used_items, parents }
}

use crate::crate_image::{Codec, Decode, fields};
fields!(Member, name, range, sig, is_fn, has_body, nested);
fields!(Nested, range, evaluated);
fields!(Head, selves, traits, inherent);
fields!(Item, module, names, kind, range, header, members, head, global, cold, nested);
impl Codec for Kind {
    fn put(&self, w: &mut Vec<u8>) { (*self as u8).put(w); }
    fn get(r: &mut Decode<'_>) -> Option<Self> {
        Some(match u8::get(r)? {
            0 => Self::Fn, 1 => Self::Decl, 2 => Self::Value, 3 => Self::Trait,
            4 => Self::Impl, 5 => Self::Use, 6 => Self::Macro, 7 => Self::Mod,
            8 => Self::Opaque, _ => return None,
        })
    }
}

use crate::crate_image::strings::{self, Pack};
strings::fields!(Member, name, range, sig, is_fn, has_body, nested);
strings::fields!(Nested, range, evaluated);
strings::fields!(Head, selves, traits, inherent);
strings::fields!(Item, module, names, kind, range, header, members, head, global, cold, nested);
impl Pack for Kind {
    fn pack(&self, w: &mut Vec<u8>, _: &mut strings::Strings) { self.put(w); }
    fn unpack(r: &mut Decode<'_>, _: &[String]) -> Option<Self> { Self::get(r) }
    fn plain_size(&self) -> usize { 1 }
}

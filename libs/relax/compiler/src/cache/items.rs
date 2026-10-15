
use crate::ast::{ItemId, ItemKind};
use crate::lexer::T;
use crate::parser::ParsedFile;
use crate::sip::Sip128;

/// A file's interface hash and the fn bodies outside it.
pub(crate) struct FileDigest {
    pub iface: u128,
    /// (item, body lo, body hi), by item
    holes: Vec<(u32, u32, u32)>,
    /// (item, the hole item whose body defines it), by item: its text is that body's
    nested_in: Vec<(u32, u32)>,
    /// token_origins sorted by position (binary search; else scanned whole)
    origins_sorted: bool,
}

impl FileDigest {
    fn hole(&self, item: u32) -> Option<(u32, u32)> {
        self.holes.binary_search_by_key(&item, |h| h.0).ok().map(|i| (self.holes[i].1, self.holes[i].2))
    }
}

fn text_range(pf: &ParsedFile) -> (u32, u32) {
    let lo = pf.toks.get(0).filter(|t| t.kind != T::Eof).map_or(0, |t| t.lo);
    // The text includes what item-level macros expanded to (it follows the authored bytes in
    // `src`): an item an invocation emits is keyed by its expansion's text like an authored
    // one, so editing inside the invocation (a `script_mod!` body, a macro_rules! call)
    // changes the keys of exactly the items it feeds. Only the authored end was used before,
    // which left every expanded item out of the interface and out of its fns' body holes.
    let hi = pf.src.len() as u32;
    (lo.min(hi), hi)
}

pub(crate) fn file_digest(pf: &ParsedFile) -> FileDigest {
    digest_into(pf, None)
}

/// The interface text of a file (holes marked `{..}`): MAKEPAD_RUN_CACHE_IFACE diagnostics.
pub(crate) fn iface_text(pf: &ParsedFile) -> Vec<u8> {
    let mut v = Vec::new();
    digest_into(pf, Some(&mut v));
    v
}

fn digest_into(pf: &ParsedFile, mut text: Option<&mut Vec<u8>>) -> FileDigest {
    let (lo, hi) = text_range(pf);
    let ast = &pf.ast;
    let n = ast.items.len();
    // items reached from the root through modules, impls and traits; the others are in bodies
    let mut reached = vec![false; n];
    let mut stack: Vec<ItemId> = ast.root_items.iter().copied().collect();
    while let Some(i) = stack.pop() {
        let Some(r) = reached.get_mut(i.0 as usize) else { continue };
        if std::mem::replace(r, true) { continue; }
        match &ast.item(i).kind {
            ItemKind::Trait(r) => stack.extend_from_slice(ast.list(ast.row(*r).items)),
            ItemKind::Impl(r) => stack.extend_from_slice(ast.list(ast.row(*r).items)),
            ItemKind::ForeignMod(r) => stack.extend_from_slice(ast.list(ast.row(*r).items)),
            ItemKind::Mod(Some(items)) => stack.extend_from_slice(ast.list(*items)),
            _ => {}
        }
    }
    // where the items in bodies were written or expanded
    let mut nested: Vec<(u32, u32)> = (0..n).filter(|&i| !reached[i]).map(|i| (pf.call_site(ast.items[i].lo), i as u32)).collect();
    nested.sort_unstable();
    // a body may define items only it can name (their text is the body's); an impl or a macro
    // there is global (the body is then interface)
    let in_body = |a: u32, b: u32| -> Option<Vec<u32>> {
        let k = nested.partition_point(|x| x.0 < a);
        let mut v = Vec::new();
        for &(p, i) in &nested[k..] {
            if p >= b { break; }
            if matches!(ast.items[i as usize].kind, ItemKind::Impl { .. } | ItemKind::MacroRules(_) | ItemKind::Mac(_)) { return None; }
            v.push(i);
        }
        Some(v)
    };
    let mut holes = Vec::new();
    let mut nested_in = Vec::new();
    {
        for (i, it) in ast.items.iter().enumerate() {
            if !reached[i] { continue; }
            // a static's initializer runs in every run (its value is read at its address, never
            // folded into another fn's code): the initializer's own code only
            if let Some(e) = match &it.kind { ItemKind::Static(_, _, e) => e.get(), _ => None } {
                let elo = ast.expr(e).lo;
                if elo >= lo && it.hi <= hi && elo >= it.lo && elo < it.hi {
                    if let Some(v) = in_body(elo, it.hi) {
                        nested_in.extend(v.into_iter().map(|x| (x, i as u32)));
                        holes.push((i as u32, elo, it.hi));
                    }
                }
                continue;
            }
            let ItemKind::Fn(sig, body, _) = &it.kind else { continue };
            let s = ast.fn_sig(*sig);
            if s.konst || s.asyncness { continue; }
            let (blo, bhi, eager) = match (body, s.lazy_body) {
                (Some(b), _) => { let b = ast.block(*b); (b.lo, b.hi, true) }
                (None, Some(r)) if r.lo > 0 => (pf.tok(r.lo - 1).lo, pf.tok(r.hi).hi, false),
                _ => continue,
            };
            if blo < lo || bhi > hi || blo < it.lo || blo >= bhi || bhi > it.hi { continue; }
            // `impl Trait` in the signature: callers' types are the body's
            let t0 = pf.toks.partition_point(|t| t.lo < it.lo);
            if pf.toks.iter_from(t0).take_while(|t| t.lo < blo).any(|t| t.kind == T::KwImpl) { continue; }
            // the items the body defines (written there or expanded from there)
            let Some(v) = (if eager { in_body(blo, bhi) } else { Some(Vec::new()) }) else { continue };
            nested_in.extend(v.into_iter().map(|x| (x, i as u32)));
            holes.push((i as u32, blo, bhi));
        }
    }
    // the text of the file's items with their attributes (and the file's inner attributes);
    // not the text between them (comments, items a cfg removed), and not their order: items
    // mean the same in any order (macros may emit a file's statics in another order), except
    // macro definitions and item macros, which are scoped by their textual order
    let attr_span = |a: &crate::ast::Attr| -> Option<(u32, u32)> {
        (a.toks.hi > a.toks.lo).then(|| (pf.tok(a.toks.lo).lo, pf.tok(a.toks.hi - 1).hi))
    };
    let mut by_pos: Vec<(u32, u32)> = holes.iter().map(|x| (x.1, x.2)).collect();
    by_pos.sort_unstable();
    let src: &[u8] = &pf.src;
    // one item's segments, holes cut out: (hash, text)
    let piece = |segs: &mut Vec<(u32, u32)>, want_text: bool| -> (u128, Vec<u8>) {
        segs.retain(|&(a, b)| a >= lo && b <= hi && a < b);
        segs.sort_unstable();
        let mut h = Sip128::new();
        let mut t = Vec::new();
        let put = |h: &mut Sip128, t: &mut Vec<u8>, a: u32, b: u32| {
            h.write(&src[a as usize..b as usize]);
            if want_text { t.extend_from_slice(&src[a as usize..b as usize]); }
        };
        let mut done = 0u32;
        for &(a, b) in segs.iter() {
            let mut at = a.max(done);
            if at >= b { continue; }
            let k = by_pos.partition_point(|x| x.1 <= at);
            for &(ha, hb) in &by_pos[k..] {
                if ha >= b { break; }
                if ha >= at {
                    put(&mut h, &mut t, at, ha);
                    h.u8(0xff);
                    if want_text { t.extend_from_slice(b"{..}"); }
                    at = hb.min(b);
                }
            }
            if at < b { put(&mut h, &mut t, at, b); }
            h.u8(0xfe);
            done = b;
        }
        if want_text { t.push(b'\n'); }
        (h.finish128(), t)
    };
    let want_text = text.is_some();
    let mut free: Vec<(u128, Vec<u8>)> = Vec::new();
    let mut ordered: Vec<(u128, Vec<u8>)> = Vec::new();
    let mut segs: Vec<(u32, u32)> = ast.root_attrs.iter().filter_map(attr_span).collect();
    ordered.push(piece(&mut segs, want_text));
    for &i in ast.root_items.iter() {
        let it = ast.item(i);
        let mut segs: Vec<(u32, u32)> = ast.list(it.attrs).iter().filter_map(attr_span).collect();
        segs.push((it.lo, it.hi));
        let p = piece(&mut segs, want_text);
        if matches!(it.kind, ItemKind::MacroRules(_) | ItemKind::Mac(_)) { ordered.push(p) } else { free.push(p) }
    }
    free.sort_unstable();
    let mut h = Sip128::new();
    h.str("iface-file");
    h.u32(free.len() as u32);
    for (x, t) in free.iter().chain(ordered.iter()) {
        h.u128(*x);
        if let Some(out) = text.as_deref_mut() { out.extend_from_slice(t); }
    }
    holes.sort_unstable_by_key(|x| x.0);
    nested_in.sort_unstable();
    nested_in.dedup_by_key(|x| x.0);
    let origins_sorted = pf.token_origins.sorted();
    let iface = h.finish128();
    FileDigest { iface, holes, nested_in, origins_sorted }
}

/// The hash of fn item `item`'s body (a static's initializer) when it is a hole (not in the
/// interface).
pub(crate) fn body_hash(pf: &ParsedFile, d: &FileDigest, item: ItemId) -> Option<u128> {
    // an item a body defines: that body's text
    let item = match d.nested_in.binary_search_by_key(&item.0, |x| x.0) {
        Ok(k) => ItemId(d.nested_in[k].1),
        Err(_) => item,
    };
    let (lo, hi) = d.hole(item.0)?;
    let mut h = Sip128::new();
    h.str("body");
    h.write(&pf.src[lo as usize..hi as usize]);
    // items the body defines are identified by their file offset (stable::def_ident: TypeId,
    // folded into this code): the body's place is part of its code then
    if d.nested_in.iter().any(|x| x.1 == item.0) {
        h.u32(lo);
    }
    Some(h.finish128())
}

/// For an item a body defines (a local struct, fn, static): that body's hash. Types made of
/// such an item (an instance's type arguments) take it into their keys.
pub(crate) fn nested_body_hash(pf: &ParsedFile, d: &FileDigest, item: ItemId) -> Option<u128> {
    d.nested_in.binary_search_by_key(&item.0, |x| x.0).ok()?;
    body_hash(pf, d, item)
}

/// Where item `item`'s code reports itself (module docs).
pub(crate) fn pos_digest(pf: &ParsedFile, d: &FileDigest, item: ItemId) -> u128 {
    let mut h = Sip128::new();
    h.str("pos");
    let Some(it) = pf.ast.items.get(item.0 as usize) else { return h.finish128() };
    let (l, c) = pf.line_col(it.lo);
    h.u64(l as u64);
    h.u64(c as u64);
    if !pf.token_input {
        return h.finish128();
    }
    // positions without an authored origin report the end of the authored file
    let (l, c) = pf.line_col(pf.src_len);
    h.u64(l as u64);
    h.u64(c as u64);
    let origins = &pf.token_origins;
    let inside = |o: &crate::source::TokenOrigin| o.lo >= it.lo && o.lo <= it.hi;
    // the rows to look at: those with lo in it.lo..=it.hi when the table is sorted, else all
    let slice = || -> Box<dyn Iterator<Item = crate::source::TokenOrigin> + '_> {
        if d.origins_sorted {
            let a = if it.lo == 0 { 0 } else { origins.count_le(it.lo - 1) };
            let b = origins.count_le(it.hi);
            Box::new(origins.iter_from(a).take(b.max(a) - a))
        } else {
            Box::new(origins.iter())
        }
    };
    // per authored file: the span the tokens come from
    let mut spans: Vec<(u32, u32, u32)> = Vec::new();
    for o in slice().filter(inside) {
        match spans.iter_mut().find(|s| s.0 == o.source.file) {
            Some(s) => { s.1 = s.1.min(o.source.lo); s.2 = s.2.max(o.source.hi); }
            None => spans.push((o.source.file, o.source.lo, o.source.hi)),
        }
    }
    // each token's place relative to its span
    for o in slice().filter(inside) {
        let k = spans.iter().position(|s| s.0 == o.source.file).unwrap_or(0);
        h.u32(k as u32);
        h.u32(o.lo - it.lo);
        h.u32(o.source.lo - spans[k].1);
        h.u32(o.source.hi.saturating_sub(spans[k].1));
    }
    // each span: its file, start line/column and its line breaks (so every relative offset
    // in it has one line/column)
    for (file, lo, hi) in spans {
        let source = if file == 0 { None } else { pf.sources.as_ref().and_then(|s| s.get(file).ok()) };
        match source {
            Some(sf) => {
                h.str(&sf.path);
                let (l, c) = sf.line_col(lo);
                h.u64(l as u64);
                h.u64(c as u64);
                let starts = sf.lines.get_or_init(|| crate::source::line_starts(&sf.src));
                let a = starts.partition_point(|&s| s <= lo);
                let b = starts.partition_point(|&s| s <= hi);
                for &s in &starts[a..b.max(a)] { h.u32(s - lo); }
            }
            None => {
                // the lowered file's own text
                h.str("");
                h.u32(lo);
                h.u32(hi);
            }
        }
    }
    h.finish128()
}

/// Per crate: its interface fingerprint -- its own files' interface hashes and its
/// dependencies' interface fingerprints. A proc-macro crate's whole fingerprint (its fn bodies
/// make the expansions of every crate that uses it).
pub(crate) fn crate_ifaces(prog: &crate::program::Program, fps: &[u128], digests: &[Option<std::sync::Arc<FileDigest>>]) -> Vec<u128> {
    let nc = prog.crates.len();
    let mut per: Vec<Sip128> = (0..nc)
        .map(|c| {
            let ci = &prog.crates[c];
            let mut h = crate::crate_image::fingerprint_head(prog.syms.str(ci.name), ci.root_path(), ci.edition, ci.proc_macro, &ci.cfg.set);
            h.str("iface");
            if ci.proc_macro {
                h.u128(fps.get(c).copied().unwrap_or(0));
            }
            h
        })
        .collect();
    // the boot's files only: a program restored with the files the lazy selection skipped
    // loaded keeps the interfaces (and code keys) of the boot that recorded its code
    for (i, d) in digests.iter().enumerate().take(prog.boot_files) {
        if prog.front_macro_fragments.contains(&i) { continue; }
        let (Some(&c), Some(d)) = (prog.file_crate.get(i), d.as_ref()) else { continue };
        if let Some(h) = per.get_mut(c as usize) {
            h.str(prog.file_paths.get(i).map_or("", |p| p.as_str()));
            h.u128(d.iface);
        }
    }
    let mut out = vec![0u128; nc];
    let mut done = vec![false; nc];
    fn go(c: usize, prog: &crate::program::Program, per: &[Sip128], out: &mut Vec<u128>, done: &mut Vec<bool>, depth: u32) -> u128 {
        if done[c] || depth > 512 {
            return out[c];
        }
        let mut named: Vec<(String, u128)> = Vec::new();
        for &(_, d) in &prog.crates[c].deps {
            if d as usize == c { continue; }
            let v = go(d as usize, prog, per, out, done, depth + 1);
            named.push((prog.syms.str(prog.crates[d as usize].name).to_string(), v));
        }
        let named = crate::crate_image::canon_deps(named);
        let mut h = per[c].clone();
        h.u32(named.len() as u32);
        for v in named { h.u128(v); }
        out[c] = h.finish128();
        done[c] = true;
        out[c]
    }
    for c in 0..nc {
        go(c, prog, &per, &mut out, &mut done, 0);
    }
    out
}

/// Every file's digest, on several threads (the cache's open).
pub(crate) fn file_digests(prog: &crate::program::Program) -> Vec<Option<std::sync::Arc<FileDigest>>> {
    struct Shared<'a>(&'a [std::sync::Arc<ParsedFile>]);
    // read-only: the parses are complete (their lazily built tables are OnceLocks)
    unsafe impl Sync for Shared<'_> {}
    let files = Shared(&prog.files);
    let n = prog.files.len();
    let threads = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut out: Vec<Option<std::sync::Arc<FileDigest>>> = (0..n).map(|_| None).collect();
    let parts: Vec<Vec<(usize, FileDigest)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                let (files, next) = (&files, &next);
                s.spawn(move || {
                    let mut v = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(pf) = files.0.get(i) else { break };
                        v.push((i, file_digest(pf)));
                    }
                    v
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    for (i, d) in parts.into_iter().flatten() {
        out[i] = Some(std::sync::Arc::new(d));
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::program::{DefId, DefKind};

    /// Identities (stable::def_ident: TypeId, cache definition keys) of a fixture's impls and
    /// their items, by def path.
    fn idents(root: &std::path::Path, src: &str) -> Vec<(String, u128)> {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let source = root.join("fixture.rs");
        std::fs::write(&source, src).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        prog.resolve_imports();
        let mut out = Vec::new();
        for i in 0..prog.defs.len() {
            let d = DefId(i as u32);
            let def = prog.def(d);
            if prog.crates[def.krate as usize].root_mod as usize >= prog.mods.len() { continue; }
            if !matches!(def.kind, DefKind::Impl | DefKind::AssocFn) || prog.syms.str(prog.crates[def.krate as usize].name) != "fixture" { continue; }
            let mut h = crate::sip::Sip128::new();
            crate::stable::def_ident(&prog, d, &mut h);
            let name = if def.kind == DefKind::Impl { format!("impl#{}", i) } else { prog.def_path(d) };
            out.push((name, h.finish128()));
        }
        std::fs::remove_file(&source).unwrap();
        out
    }

    /// Impls with identical headers (two `impl Twin` blocks, a macro's impls) get different
    /// identities, and the identities do not move when text above them changes.
    #[test]
    fn identical_impl_headers_are_told_apart_and_position_free() {
        let body = "pub struct Twin;\nimpl Twin { pub fn a(&self) -> u32 { 1 } }\nimpl Twin { pub fn b(&self) -> u32 { 2 } }\nmacro_rules! twin { ($n:ident) => { impl Twin { pub fn $n(&self) -> u32 { 3 } } } }\ntwin!(c);\ntwin!(d);\n";
        // one path for both versions (the crate's identity is its name and root file)
        let root = std::env::temp_dir().join(format!("relax-impl-twins-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let a = idents(&root, body);
        let impls: Vec<u128> = a.iter().filter(|x| x.0.starts_with("impl#")).map(|x| x.1).collect();
        assert_eq!(impls.len(), 4, "{:?}", a);
        let mut uniq = impls.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), 4, "identical impl headers share an identity: {:?}", a);
        let mut fns: Vec<u128> = a.iter().filter(|x| !x.0.starts_with("impl#")).map(|x| x.1).collect();
        fns.sort();
        fns.dedup();
        assert_eq!(fns.len(), 4, "{:?}", a);
        // text above every item: same identities
        let b = idents(&root, &format!("// a comment that moves everything below it\n\n{}", body));
        std::fs::remove_dir(&root).unwrap();
        let ids = |v: &[(String, u128)]| { let mut x: Vec<u128> = v.iter().map(|x| x.1).collect(); x.sort(); x };
        assert_eq!(ids(&a), ids(&b));
    }
}

//! `makepad check` reuses the verdict of every checked crate whose inputs did not change since a
//! check that passed: its diagnostics, as that check printed them, without checking the crate
//! again (as cargo check keeps an unchanged crate's metadata).
//!
//! A crate's key (`Reuse::open_in`) is what its check's output rests on: the Relax build and its
//! options, every `MAKEPAD_RUN_*` variable (the command line's lint flags among them), the checked
//! crates and the root, the crate as rustc is invoked on it (name, root, edition, cfg with the
//! features, binary or not, test harness, its dependencies by alias) and the text of every file of
//! it; and of each crate of its dependency closure, the text when that crate is not checked (std,
//! a registry crate), the text of its whole closure when it is a proc-macro crate (its code makes
//! the expansions), and its identity when it is checked too. The file hashes are the front's
//! (ParsedFile::source_hash: the graph certificate's memo on a warm start).
//!
//! What the key cannot name travels with the verdict and is checked before the verdict is used:
//! the interface of each checked crate of the closure (cache/items.rs: its items outside the fn
//! bodies that are holes; a file the load skipped by its text), the macro inputs outside the
//! source that the crate and its closure read (include_str!, include!, env!: session sip.rs late
//! inputs, which lazily parsed bodies read after the load), and the text of every other crate a
//! stored diagnostic points into. So a body edit in a dependency checks that dependency again,
//! not every crate above it.
//!
//! An interface that changed is compared by node (`KIND_TABLE` holds each checked crate's
//! interface as the nodes of the code cache's item graph, cache/deps.rs check_nodes, those of the
//! fragments its item macros expanded into among them): what a crate's check takes from another
//! crate reaches it through a name its text or its macro expansions write (the verdict holds them:
//! cache.rs text_names), or through a node that names such a name (deps.rs: name resolution finds
//! an item only under a name written; a type's layout, a signature, a constant's value come from
//! the pieces of the items named, whose own names reach what they are made of; trait selection
//! finds an impl under its self type's names, and one that applies to types its self type does
//! not name -- a blanket impl, one for a primitive, a reference or an alias -- also under its
//! trait's name, its items' names, its trait's arguments' names and its trait's items' names). So
//! the names under which a changed, added or removed node is found are dirty, and so, transitively,
//! are those of every node whose piece names a dirty name, in whichever crate (a check of a crate
//! that sees two others finds an impl of one for a type the other's items name) (`Dirty`). A
//! verdict stands while its crate's text writes no dirty name. What a name cannot show is coarse:
//! a changed always node (inner attributes, glob and `as _` imports, an item macro outside a
//! module, one defining an impl or a macro where no name finds it, a skipped file) or a changed
//! head (the crate's dependencies by alias) checks every crate that can see it again; a node
//! invoking a macro whose expansion it does not have is dirty when a crate it can see owns a dirty
//! node. A doc comment is in no piece: editing one checks only its crate again.
//!
//! A verdict (`KIND_CRATE`) is the crate's diagnostics as the check rendered them, each with where
//! the sort places it (check_order.rs: its stage and its group, by path) and whether only a check
//! whose required analyses passed reports it (`LATE`) or only one whose well-formedness checks ran
//! (`WF`). Only a check with no error, and nothing accepted only in relaxed Rust, stores verdicts.
//! A check replays the hits' diagnostics among its own (the sort merges them, one of each text)
//! and runs the per-crate passes on the other crates only (check.rs); the cross-crate checks
//! (duplicate symbols, the allocator and lang items, the entry point, monomorphization) and the
//! check-wide gates still see every crate.
//!
//! When every crate hits and a passing check of the same keys stored its run record (`KIND_RUN`:
//! it printed no signature error, of any crate), the check prints the stored diagnostics right
//! after the load (run.rs), before declaration collection: nothing a check prints in between
//! rests on anything but the keys' inputs and those errors.
//!
//! Such a check also stores what it printed as its early record (`Early`), under a key known
//! before the load: the command line, the unit graph and the load's graph certificate (the inputs
//! and module probes the front's images rest on). The next check of that command line whose
//! certificate is current prints it right after the build scripts, with no load at all.
use crate::cache::CheckNode;
use crate::check_order::{Entry, GroupAt, Out, Phase, Stage};
use crate::program::{Program, NO_DEF};
use crate::sip::Sip128;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const VERSION: u32 = 3;
const KIND_CRATE: u32 = 1;
const KIND_RUN: u32 = 2;
/// a checked crate's interface by node (`Table`), under its identity and interface hash
const KIND_TABLE: u32 = 3;
/// a whole check's output under a key known before the load (`Early`)
const KIND_EARLY: u32 = 4;
/// a late lint: printed only when the required analyses passed (check_order.rs Stage::required)
pub(crate) const LATE: u8 = 1;
/// reported by the well-formedness checks, which run only when nothing failed before them
pub(crate) const WF: u8 = 2;

/// A stored diagnostic.
struct Stored { phase: Phase, step: u8, flags: u8, seq: u32, group: GroupAt, text: String }

/// A decoded verdict.
struct Verdict {
    /// (crate name, kind, name, content hash): the late inputs it rests on
    late: Vec<(String, u8, String, u128)>,
    /// (crate identity, its text): the other crates its diagnostics point into
    refs: Vec<(u128, u128)>,
    /// (crate identity, its interface): the checked crates of its closure, as it was checked
    deps: Vec<(u128, u128)>,
    /// the names the crate's text and its macro expansions write, sorted (cache.rs text_names)
    names: Vec<u64>,
    entries: Vec<Stored>,
}

/// A checked crate's interface by node (deps.rs CheckNode), as a stored verdict above it saw it:
/// its head (identity, dependencies by alias) and its nodes, sorted by piece.
struct Table {
    head: u128,
    nodes: Vec<TNode>,
}

/// A node of a `Table`: its piece, NODE_* flags, the names it is found under, a trait impl's
/// trait and the other names that find it when it applies to types its self type does not name
/// (CheckNode::wide).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct TNode {
    piece: u128,
    flags: u8,
    keys: Vec<u64>,
    wide: Vec<u64>,
}

/// reached by every node of the crates that can see it (CheckNode::always; a skipped file)
const NODE_ALWAYS: u8 = 1;
/// a trait impl whose self type's head is a named path (CheckNode::named_head)
const NODE_NAMED_HEAD: u8 = 2;

impl Table {
    /// A crate's table from its `head` and its nodes now.
    fn of(head: u128, nodes: &[CheckNode]) -> Table {
        let mut nodes: Vec<TNode> = nodes.iter().map(|x| TNode {
            piece: x.piece,
            flags: if x.always { NODE_ALWAYS } else { 0 } | if x.named_head { NODE_NAMED_HEAD } else { 0 },
            keys: x.keys.clone(),
            wide: x.wide.clone(),
        }).collect();
        nodes.sort_unstable();
        Table { head, nodes }
    }
}

/// A file of a checked crate that a checked crate depends on, now: its path, the file (None: the
/// load skipped it; its text hash stands for its interface), its interface hash.
struct CurFile {
    path: String,
    file: Option<usize>,
    iface: u128,
}

/// One check's reuse: the checked crates' keys, the hits and their diagnostics.
pub(crate) struct Reuse {
    path: PathBuf,
    write: bool,
    checked: Vec<u32>,
    /// parallel to `checked`
    keys: Vec<u128>,
    hit: Vec<bool>,
    /// a hit's verdict as stored (a rewrite of the store keeps it)
    raw: Vec<Option<Vec<u8>>>,
    closures: Vec<Vec<u32>>,
    stored: Vec<Stored>,
    run_key: u128,
    run_hit: bool,
    /// by crate: identity, text
    idents: Vec<u128>,
    texts: Vec<u128>,
    /// by crate, of the checked crates some checked crate depends on: head, interface hash, files
    /// (CurFile), whether the store holds that interface's table
    heads: Vec<u128>,
    ifaces: Vec<u128>,
    cur: Vec<Vec<CurFile>>,
    table_stored: Vec<bool>,
    /// by file: the digests of `cur`'s files (their nodes: CheckNode)
    digests: Vec<Option<std::sync::Arc<crate::cache::FileDigest>>>,
    /// by crate: its interface's table now, once made (`Dirty::of`, `record`)
    tables: Vec<Option<Table>>,
    /// parallel to `checked`: (identity, interface) of the checked crates of its closure, now
    deps: Vec<Vec<(u128, u128)>>,
    /// parallel to `checked`: a hit whose verdict was checked against interfaces that changed since
    /// (stored again with today's)
    restore: Vec<bool>,
    store_entries: usize,
    /// run.rs: no signature error was printed (a run record may be stored)
    pub(crate) sig_clean: bool,
    pub(crate) t_keys: std::time::Duration,
    /// files the load skipped: of the checked crates, of the others
    pub(crate) skipped: (usize, usize),
    /// every crate's files, (path, content hash): the texts the keys rest on (Early::record)
    files: Vec<(String, u128)>,
    /// the hits' late inputs
    late: Vec<(String, u8, String, u128)>,
    /// `record` stored a verdict of every crate that did not hit, and the run record: the next
    /// check of the same inputs reuses all (`Early`: its output can be stored now)
    complete: bool,
    /// the diagnostics of the verdicts `record` stored
    recorded: Vec<Stored>,
}

/// What a check's crates' keys rest on beyond the crates: the Relax build and options, the
/// variables that steer the check (every `MAKEPAD_RUN_*` one but those naming the run a check
/// runs under, run.rs RUN_STATE_VARS).
fn env_key(prog: &Program) -> u128 {
    let mut h = Sip128::new();
    h.str("relax-check-env");
    h.u32(VERSION);
    h.u128(crate::codegen_id::id());
    h.u128(prog.session.options.fingerprint());
    let mut vars: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.to_string_lossy().into_owned())))
        .filter(|(k, _)| k.starts_with("MAKEPAD_RUN_") && !crate::run::RUN_STATE_VARS.contains(&k.as_str()))
        .collect();
    vars.sort();
    h.u32(vars.len() as u32);
    for (k, v) in vars { h.str(&k); h.str(&v); }
    h.finish128()
}

/// The crate's identity: as rustc is invoked on it.
fn ident(prog: &Program, c: u32) -> u128 {
    let ci = &prog.crates[c as usize];
    crate::crate_image::fingerprint_head(prog.syms.str(ci.name), ci.root_path(), ci.edition, ci.proc_macro, &ci.cfg.set).finish128()
}

/// The files the load skipped: (crate, path, content hash). They have no parse: their text is
/// their input (the selection read them already).
fn skipped_files(prog: &Program) -> Vec<(u32, String, u128)> {
    prog.skipped_files.iter().filter_map(|(m, (path, ..))| {
        let c = prog.mods.get(*m as usize)?.krate;
        // (an unreadable file: its absence is the input)
        Some((c, path.clone(), crate::vfs::read(path).map_or(0, |src| crate::crate_image::file_hash(path, &src))))
    }).collect()
}

/// Each crate's files: (path, content hash) by path, with the `skipped` ones.
fn crate_files(prog: &Program, skipped: &[(u32, String, u128)]) -> Vec<Vec<(String, u128)>> {
    let mut files: Vec<Vec<(String, u128)>> = vec![Vec::new(); prog.crates.len()];
    for f in 0..prog.files.len() {
        if prog.front_macro_fragments.contains(&f) { continue; }
        let (Some(&c), Some(path)) = (prog.file_crate.get(f), prog.file_paths.get(f)) else { continue };
        let Some(v) = files.get_mut(c as usize) else { continue };
        v.push((path.clone(), prog.files[f].source_hash(path)));
    }
    for (c, path, h) in skipped {
        if let Some(v) = files.get_mut(*c as usize) { v.push((path.clone(), *h)); }
    }
    for v in &mut files { v.sort_unstable(); v.dedup(); }
    files
}

/// The files of the crates `of` marks, by crate: their interfaces (CurFile), by path; and the
/// loaded ones' digests, by file. The fragments (the expansions of item macros the load expanded
/// after the file's parse, under its path) are files of the crate's interface too: an edit of a
/// macro another crate defines changes them.
fn iface_files(prog: &Program, of: &[bool], skipped: &[(u32, String, u128)]) -> (Vec<Vec<CurFile>>, Vec<Option<std::sync::Arc<crate::cache::FileDigest>>>) {
    let mut files: Vec<Vec<CurFile>> = (0..prog.crates.len()).map(|_| Vec::new()).collect();
    let want: Vec<usize> = (0..prog.files.len())
        .filter(|f| prog.file_crate.get(*f).is_some_and(|&c| of.get(c as usize).copied().unwrap_or(false)))
        .collect();
    let digests = crate::cache::file_digests_of(prog, &want);
    for f in want {
        let (Some(&c), Some(path), Some(Some(d))) = (prog.file_crate.get(f), prog.file_paths.get(f), digests.get(f)) else { continue };
        files[c as usize].push(CurFile { path: path.clone(), file: Some(f), iface: d.iface });
    }
    for (c, path, h) in skipped {
        if of.get(*c as usize).copied().unwrap_or(false) {
            files[*c as usize].push(CurFile { path: path.clone(), file: None, iface: *h });
        }
    }
    for v in &mut files {
        v.sort_unstable_by(|a, b| (&a.path, a.iface).cmp(&(&b.path, b.iface)));
        v.dedup_by(|a, b| a.path == b.path && a.iface == b.iface);
    }
    (files, digests)
}

/// A crate's head: its identity and its dependencies by alias.
fn head_hash(prog: &Program, c: u32, idents: &[u128]) -> u128 {
    let ci = &prog.crates[c as usize];
    let mut h = Sip128::new();
    h.str("check-head");
    h.u128(idents[c as usize]);
    let mut deps: Vec<(&str, u128)> = ci.deps.iter().filter(|(_, d)| *d != c).map(|&(a, d)| (prog.syms.str(a), idents[d as usize])).collect();
    deps.sort_unstable();
    deps.dedup();
    h.u32(deps.len() as u32);
    for (a, d) in deps { h.str(a); h.u128(d); }
    h.finish128()
}

/// A crate's hash over its head and `files` (path, hash).
fn crate_hash<'a>(what: &str, head: u128, files: impl ExactSizeIterator<Item = (&'a str, u128)>) -> u128 {
    let mut h = Sip128::new();
    h.str(what);
    h.u128(head);
    h.u32(files.len() as u32);
    for (p, x) in files { h.str(p); h.u128(x); }
    h.finish128()
}

/// The store key of a checked crate's interface table.
fn table_key(ident: u128, iface: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("relax-check-table");
    h.u128(ident);
    h.u128(iface);
    h.finish128()
}

/// The text of crate `c` and of every crate of its closure (a proc-macro crate's code).
fn deep_text(prog: &Program, c: u32, texts: &[u128]) -> u128 {
    let mut v: Vec<u128> = crate::check::dependency_closure(prog, c).iter().map(|&d| texts[d as usize]).collect();
    v.sort_unstable();
    let mut h = Sip128::new();
    h.str("deep-text");
    h.u128(texts[c as usize]);
    h.u32(v.len() as u32);
    for x in v { h.u128(x); }
    h.finish128()
}

/// The current value of a late input (as the front validates its images: cache.rs).
fn late_current(c: &str, kind: u8, name: &str, hash: u128) -> bool {
    let content = if kind == crate::sip::LATE_FILE { crate::vfs::read(name).ok() } else { crate::crates::crate_env_named(c, name).map(String::into_bytes) };
    crate::sip::late_hash(content.as_deref()) == hash
}

impl Reuse {
    /// The reuse of a one-shot check of `checked` rooted at `root`, in the cache's folder. None:
    /// the cache is off, or verifying (every crate is checked).
    pub(crate) fn open(prog: &Program, root: u32, checked: &[u32]) -> Option<Reuse> {
        let mode = crate::cache::mode_from_env(&prog.session.options)?;
        if mode == crate::cache::Mode::Verify { return None; }
        Some(Self::open_in(prog, root, checked, &crate::dirs::relax_dir().join("cache"), mode == crate::cache::Mode::On))
    }

    /// `open` with the store in `dir`; `write`: this check stores what it finds.
    pub(crate) fn open_in(prog: &Program, root: u32, checked: &[u32], dir: &Path, write: bool) -> Reuse {
        let t0 = std::time::Instant::now();
        let n = prog.crates.len();
        let mut checked = checked.to_vec();
        checked.sort_unstable();
        checked.dedup();
        let idents: Vec<u128> = (0..n as u32).map(|c| ident(prog, c)).collect();
        let heads: Vec<u128> = (0..n as u32).map(|c| head_hash(prog, c, &idents)).collect();
        let mut is_checked = vec![false; n];
        for &c in &checked { is_checked[c as usize] = true; }
        let closures: Vec<Vec<u32>> = checked.iter().map(|&c| crate::check::dependency_closure(prog, c)).collect();
        // the checked crates some checked crate depends on: their interfaces
        let mut iface_of = vec![false; n];
        for d in closures.iter().flatten() {
            if is_checked[*d as usize] && !prog.crates[*d as usize].proc_macro { iface_of[*d as usize] = true; }
        }
        let skipped = skipped_files(prog);
        let text_files = crate_files(prog, &skipped);
        let texts: Vec<u128> = (0..n).map(|c| crate_hash("check-text", heads[c], text_files[c].iter().map(|(p, h)| (p.as_str(), *h)))).collect();
        let (cur, digests) = iface_files(prog, &iface_of, &skipped);
        let ifaces: Vec<u128> = (0..n).map(|c| if iface_of[c] { crate_hash("check-iface", heads[c], cur[c].iter().map(|f| (f.path.as_str(), f.iface))) } else { 0 }).collect();
        let mut deep: HashMap<u32, u128> = HashMap::new();
        let env = env_key(prog);
        let set = {
            let mut h = Sip128::new();
            h.str("relax-check-set");
            h.u128(idents[root as usize]);
            let mut v: Vec<u128> = checked.iter().map(|&c| idents[c as usize]).collect();
            v.sort_unstable();
            h.u32(v.len() as u32);
            for x in v { h.u128(x); }
            h.finish128()
        };
        let keys: Vec<u128> = checked.iter().zip(&closures).map(|(&c, closure)| {
            let ci = &prog.crates[c as usize];
            let mut h = Sip128::new();
            h.str("relax-check-crate");
            h.u128(env);
            h.u128(set);
            h.u128(texts[c as usize]);
            h.u8(ci.is_bin as u8);
            h.u8(ci.harness as u8);
            let mut v: Vec<u128> = closure.iter().map(|&d| {
                if prog.crates[d as usize].proc_macro { *deep.entry(d).or_insert_with(|| deep_text(prog, d, &texts)) }
                else if iface_of[d as usize] { idents[d as usize] }
                else { texts[d as usize] }
            }).collect();
            v.sort_unstable();
            h.u32(v.len() as u32);
            for x in v { h.u128(x); }
            h.finish128()
        }).collect();
        let deps: Vec<Vec<(u128, u128)>> = closures.iter().map(|closure| {
            let mut v: Vec<(u128, u128)> = closure.iter().filter(|&&d| iface_of[d as usize]).map(|&d| (idents[d as usize], ifaces[d as usize])).collect();
            v.sort_unstable();
            v
        }).collect();
        let run_key = {
            let mut h = Sip128::new();
            h.str("relax-check-run");
            h.u128(set);
            let mut v = keys.clone();
            v.sort_unstable();
            for x in v { h.u128(x); }
            h.finish128()
        };
        let path = dir.join(format!("check-{:016x}.rcs", set as u64));
        let trace = std::env::var_os("MAKEPAD_RUN_CHECK_REUSE_TRACE").is_some();
        let mut r = Reuse {
            path, write, hit: vec![false; checked.len()], raw: vec![None; checked.len()], keys, checked, closures,
            stored: Vec::new(), run_key, run_hit: false, idents, texts, heads, ifaces, cur, table_stored: vec![false; n],
            digests, tables: (0..n).map(|_| None).collect(), deps, restore: Vec::new(), store_entries: 0, sig_clean: false, t_keys: Default::default(),
            skipped: (skipped.iter().filter(|s| is_checked[s.0 as usize]).count(), skipped.iter().filter(|s| !is_checked[s.0 as usize]).count()),
            files: Vec::new(), late: Vec::new(), complete: false, recorded: Vec::new(),
        };
        r.restore = vec![false; r.checked.len()];
        // MAKEPAD_RUN_CHECK_REUSE_TRACE: each crate's key parts (two checks' traces diff to what changed)
        if trace {
            for c in 0..n {
                let j = r.checked.binary_search(&(c as u32)).ok();
                eprintln!("check-reuse-trace: {} checked={} text={:032x} iface={:032x} key={}", prog.syms.str(prog.crates[c].name), j.is_some(), r.texts[c], r.ifaces[c], j.map_or(String::new(), |j| format!("{:032x}", r.keys[j])));
                for (p, h) in &text_files[c] { eprintln!("check-reuse-trace:   {} {:032x}", p, h); }
                for f in &r.cur[c] { eprintln!("check-reuse-trace:   {} {:032x} iface", f.path, f.iface); }
            }
            for s in &skipped { eprintln!("check-reuse-trace: skipped {} {}", prog.syms.str(prog.crates[s.0 as usize].name), s.1); }
            // (=<path part>[,<path part>..]: the interface text of the files whose path holds one)
            if let Some(parts) = std::env::var("MAKEPAD_RUN_CHECK_REUSE_TRACE").ok().filter(|v| v.len() > 1) {
                for (f, p) in prog.file_paths.iter().enumerate().filter(|(_, p)| parts.split(',').any(|q| !q.is_empty() && p.contains(q))) {
                    let pf = &prog.files[f];
                    let lazy = pf.ast.items.iter().filter(|it| matches!(&it.kind, crate::ast::ItemKind::Fn(sig, None, _) if pf.ast.fn_sig(*sig).lazy_body.is_some())).count();
                    eprintln!("check-reuse-trace: iface text of {} ({} items, {} lazy bodies, src {} of {}):\n{}", p, pf.ast.items.len(), lazy, pf.src_len, pf.src.len(), String::from_utf8_lossy(&crate::cache::file_iface_text(prog, f)));
                }
            }
        }
        if let Some(store) = crate::store::Store::open(&r.path) {
            r.store_entries = store.index.len();
            for c in 0..n {
                if iface_of[c] { r.table_stored[c] = matches!(store.get_checked(table_key(r.idents[c], r.ifaces[c])), Some((KIND_TABLE, _))); }
            }
            let by_ident: HashMap<u128, u32> = r.idents.iter().enumerate().map(|(c, &i)| (i, c as u32)).collect();
            let mut late_memo: HashMap<(String, u8, String, u128), bool> = HashMap::new();
            // verdicts checked against interfaces that changed since: (j, verdict, the changed
            // (crate, interface it saw))
            let mut changed: Vec<(usize, Verdict, Vec<(u32, u128)>)> = Vec::new();
            for j in 0..r.checked.len() {
                let Some((KIND_CRATE, p)) = store.get_checked(r.keys[j]) else { continue };
                let Some(v) = decode(p) else { continue };
                let late_ok = v.late.iter().all(|(c, k, name, h)| *late_memo.entry((c.clone(), *k, name.clone(), *h)).or_insert_with(|| late_current(c, *k, name, *h)));
                let refs_ok = v.refs.iter().all(|(i, t)| by_ident.get(i).is_some_and(|&c| r.texts[c as usize] == *t));
                // (the key holds the checked crates of the closure by identity: these are they)
                if !late_ok || !refs_ok || v.deps.len() != r.deps[j].len() { continue; }
                let mut since: Vec<(u32, u128)> = Vec::new();
                for ((i, was), (i2, now)) in v.deps.iter().zip(&r.deps[j]) {
                    if i != i2 { since.push((u32::MAX, 0)); break; }
                    if was != now { since.push((by_ident[i], *was)); }
                }
                r.raw[j] = Some(p.to_vec());
                if since.is_empty() {
                    r.hit[j] = true;
                    r.late.extend(v.late);
                    r.stored.extend(v.entries);
                } else {
                    changed.push((j, v, since));
                }
            }
            if !changed.is_empty() {
                let dirty = Dirty::of(prog, &mut r, &store, &changed, trace);
                for (j, v, since) in changed {
                    let c = r.checked[j];
                    let ok = since.iter().all(|&(d, was)| d != u32::MAX && !dirty.unknown.contains(&(d, was)))
                        && !dirty.reaches(&r.closures[j], &v.names);
                    if trace { eprintln!("check-reuse-trace: {} checked against changed interfaces: {}", prog.syms.str(prog.crates[c as usize].name), if ok { "stands" } else { "checked again" }); }
                    if ok {
                        let v = Verdict { deps: r.deps[j].clone(), ..v };
                        r.raw[j] = Some(encode(&v));
                        r.hit[j] = true;
                        r.restore[j] = true;
                        r.late.extend(v.late);
                        r.stored.extend(v.entries);
                    } else {
                        r.raw[j] = None;
                    }
                }
            }
            r.run_hit = matches!(store.get_checked(r.run_key), Some((KIND_RUN, _)));
        }
        r.files = text_files.into_iter().flatten().collect();
        r.t_keys = t0.elapsed();
        r
    }

    /// The hits whose verdicts were checked against interfaces that changed since.
    pub(crate) fn restored(&self) -> usize { self.restore.iter().filter(|a| **a).count() }

    /// The nodes now of each crate of `which` (CheckNode; a file the load skipped is one always
    /// node of its text), by crate.
    fn nodes_now(&self, prog: &Program, which: &[u32], user: &crate::cache::UserNames) -> Vec<(u32, Vec<CheckNode>)> {
        let want: Vec<usize> = which.iter().flat_map(|&c| self.cur[c as usize].iter().filter_map(|f| f.file)).collect();
        let mut by_file = crate::cache::check_nodes_of(prog, &want, &self.digests, &user.macros);
        which.iter().map(|&c| {
            let mut v = Vec::new();
            for f in &self.cur[c as usize] {
                match f.file {
                    Some(k) => v.extend(by_file.get_mut(k).and_then(Option::take).unwrap_or_default()),
                    None => {
                        let mut h = Sip128::new();
                        h.str("check-skipped-file");
                        h.str(&f.path);
                        h.u128(f.iface);
                        v.push(CheckNode { piece: h.finish128(), keys: Vec::new(), names: Vec::new(), wide: Vec::new(), named_head: true, trait_def: false, always: true, coarse: false });
                    }
                }
            }
            (c, v)
        }).collect()
    }

    /// Whether checked crate `c`'s verdict is reused.
    pub(crate) fn is_hit(&self, c: u32) -> bool {
        self.checked.binary_search(&c).is_ok_and(|j| self.hit[j])
    }

    pub(crate) fn hits(&self) -> usize { self.hit.iter().filter(|h| **h).count() }

    pub(crate) fn crates(&self) -> usize { self.checked.len() }

    /// Every crate hit and a passing check of the same keys stored its run record (run.rs: the
    /// check ends after the load).
    pub(crate) fn full(&self) -> bool { self.run_hit && self.hit.iter().all(|h| *h) }

    /// The hits' diagnostics: the late lints only when `late_ok`, the well-formedness checks'
    /// only when they ran (`wf_ran`), as the check that stored them would have reported them.
    pub(crate) fn replayed(&self, late_ok: bool, wf_ran: bool) -> Vec<Entry> {
        self.stored.iter().filter(|s| (late_ok || s.flags & LATE == 0) && (wf_ran || s.flags & WF == 0)).map(|s| Entry {
            stage: Stage::new(s.phase, s.step), seq: s.seq, out: Out::Warning(s.text.clone()), group_at: Some(s.group.clone()),
        }).collect()
    }

    /// A check whose every crate is unchanged (`full`), or one that stored the verdicts of all
    /// the others (`complete`): the stored diagnostics in the order the check that stored them
    /// printed them (`print_reused_texts` prints them).
    pub(crate) fn reused_texts(&self, prog: &Program) -> Vec<String> {
        let all = self.stored.iter().chain(&self.recorded).map(|s| Entry {
            stage: Stage::new(s.phase, s.step), seq: s.seq, out: Out::Warning(s.text.clone()), group_at: Some(s.group.clone()),
        }).collect();
        crate::check_order::sort(prog, all).into_iter()
            .filter_map(|e| match e.out { Out::Warning(s) => Some(s), _ => None }).collect()
    }

    /// A passing check's diagnostics `entries` (its own: no replayed one; `flags` LATE / WF,
    /// parallel) stored as the verdicts of the crates that did not hit, and its run record
    /// when `sig_clean`. Nothing when a diagnostic belongs to no checked crate, or anything was
    /// accepted only in relaxed Rust (its report is the whole check's).
    pub(crate) fn record(&mut self, prog: &Program, entries: &[Entry], flags: &[u8]) {
        if !self.write { return; }
        let verbose = prog.session.options.verbose;
        if relax_types::borrowck::relaxed_pending() || crate::relaxed::pending() {
            if verbose { eprintln!("makepad check: verdicts not stored: sites accepted only in relaxed Rust"); }
            return;
        }
        let mut by_path: HashMap<&str, u32> = HashMap::new();
        for (f, p) in prog.file_paths.iter().enumerate() {
            if let Some(&c) = prog.file_crate.get(f) { by_path.entry(p.as_str()).or_insert(c); }
        }
        let mut per: Vec<Vec<usize>> = vec![Vec::new(); self.checked.len()];
        for (i, e) in entries.iter().enumerate() {
            let text = match &e.out { Out::Warning(s) => s.as_str(), _ => return };
            // whose check reported it: the definition it was reported in, else its place's crate
            let producer = if e.stage.group != NO_DEF && (e.stage.group.0 as usize) < prog.defs.len() { Some(prog.def(e.stage.group).krate) }
                else { crate::check_order::path_of(text).and_then(|p| by_path.get(p).copied()) };
            let Some(j) = producer.and_then(|k| self.checked.binary_search(&k).ok()) else {
                if verbose { eprintln!("makepad check: verdicts not stored: a diagnostic of no checked crate: {}", text.lines().next().unwrap_or("")); }
                return;
            };
            // (a hit's own diagnostics are in its stored verdict already)
            if !self.hit[j] { per[j].push(i); }
        }
        let stages: Vec<Stage> = entries.iter().map(|e| e.stage).collect();
        let groups = crate::check_order::groups_at(prog, &stages);
        let late = crate::sip::late_inputs();
        // the names the text of each crate that did not hit writes (its fragments' and expansions'
        // among them)
        let fresh: Vec<usize> = (0..self.checked.len()).filter(|&j| !self.hit[j]).collect();
        let mut slot: Vec<Option<usize>> = vec![None; prog.crates.len()];
        for (k, &j) in fresh.iter().enumerate() { slot[self.checked[j] as usize] = Some(k); }
        let mut files: Vec<Vec<usize>> = vec![Vec::new(); fresh.len()];
        for (f, &c) in prog.file_crate.iter().enumerate() {
            if let Some(k) = slot.get(c as usize).copied().flatten() { files[k].push(f); }
        }
        let mut text_names = crate::cache::text_names(prog, &files).into_iter();
        let mut new: Vec<crate::pack::NewEntry> = Vec::new();
        for j in 0..self.checked.len() {
            if self.hit[j] { continue; }
            let written = text_names.next().unwrap_or_default();
            let c = self.checked[j];
            // the late inputs of the crate and of its closure (by crate name, as the front notes them)
            let names: std::collections::HashSet<&str> = std::iter::once(c).chain(self.closures[j].iter().copied())
                .map(|k| prog.syms.str(prog.crates[k as usize].name)).collect();
            let mine: Vec<&(String, u8, String, u128)> = late.iter().filter(|(k, ..)| names.contains(k.as_str())).collect();
            // the other crates its diagnostics point into (their text: positions and the mod chains)
            let mut refs: Vec<(u128, u128)> = Vec::new();
            for &i in &per[j] {
                let Out::Warning(text) = &entries[i].out else { continue };
                let group = match &groups[i] { GroupAt::Path(p, ..) => Some(p.as_str()), _ => None };
                for p in crate::check_order::paths_named(text).into_iter().chain(group) {
                    if let Some(&k) = by_path.get(p) {
                        if k != c { refs.push((self.idents[k as usize], self.texts[k as usize])); }
                    }
                }
            }
            refs.sort_unstable();
            refs.dedup();
            let entries: Vec<Stored> = per[j].iter().filter_map(|&i| {
                let e = &entries[i];
                let Out::Warning(text) = &e.out else { return None };
                Some(Stored { phase: e.stage.phase, step: e.stage.step, flags: flags[i], seq: e.seq, group: groups[i].clone(), text: text.clone() })
            }).collect();
            let late = mine.into_iter().cloned().collect();
            let v = Verdict { late, refs, deps: self.deps[j].clone(), names: written, entries };
            new.push(crate::pack::NewEntry { key: self.keys[j], kind: KIND_CRATE, payload: encode(&v) });
            self.recorded.extend(v.entries);
        }
        // a hit checked against interfaces that changed since: stored with today's
        for j in 0..self.checked.len() {
            if !self.restore[j] { continue; }
            if let Some(p) = self.raw[j].take() { new.push(crate::pack::NewEntry { key: self.keys[j], kind: KIND_CRATE, payload: p }); }
        }
        if self.sig_clean {
            new.push(crate::pack::NewEntry { key: self.run_key, kind: KIND_RUN, payload: VERSION.to_le_bytes().to_vec() });
        }
        if new.is_empty() { return; }
        // a store that holds many checks' worth of verdicts starts again from this check's
        let drop_all = self.store_entries > 8 * self.checked.len() + 64;
        if drop_all {
            for j in 0..self.checked.len() {
                if let Some(p) = self.raw[j].take() { new.push(crate::pack::NewEntry { key: self.keys[j], kind: KIND_CRATE, payload: p }); }
            }
        }
        // the interfaces the verdicts were checked against, by node (a later check compares them)
        let want: Vec<u32> = (0..self.cur.len() as u32).filter(|&c| self.ifaces[c as usize] != 0 && (drop_all || !self.table_stored[c as usize])).collect();
        let missing: Vec<u32> = want.iter().copied().filter(|&c| self.tables[c as usize].is_none()).collect();
        if !missing.is_empty() {
            let user = crate::cache::user_names(prog);
            for (c, v) in self.nodes_now(prog, &missing, &user) { self.tables[c as usize] = Some(Table::of(self.heads[c as usize], &v)); }
        }
        for c in want {
            if let Some(t) = &self.tables[c as usize] {
                new.push(crate::pack::NewEntry { key: table_key(self.idents[c as usize], self.ifaces[c as usize]), kind: KIND_TABLE, payload: encode_table(t) });
            }
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let r = dir.parent().map_or(Ok(()), crate::pack::ensure_private_dir).and_then(|_| crate::pack::ensure_private_dir(dir))
            .and_then(|_| crate::store::append(&self.path, new, drop_all).map_err(|e| e.to_string()));
        self.complete = r.is_ok() && self.sig_clean;
        if verbose { eprintln!("makepad check: verdicts stored ({:?})", r.err()); }
    }

    /// `record` stored every verdict and the run record (`complete`).
    pub(crate) fn complete(&self) -> bool { self.complete }
}

/// What changed in the interfaces that verdicts checked against older ones of saw (module doc):
/// the names under which a changed node is found, and transitively those of every node naming a
/// dirty name.
struct Dirty {
    /// (crate, interface) whose table the store does not hold: what changed is not known
    unknown: HashSet<(u32, u128)>,
    /// by crate: a change no name shows (its head, an always node): every crate that can see it
    /// is checked again
    coarse: Vec<bool>,
    names: crate::fxhash::HashSet<u64>,
}

/// The nodes that differ between two tables' sorted nodes: a piece the other lacks, or holds a
/// different number of times or with other flags. (The same piece is the same node whatever names
/// it is found under now: an inherent impl's header gains its new items' names, nodes of their own.)
fn changed_nodes<'a>(old: &'a [TNode], new: &'a [TNode], mut f: impl FnMut(&'a TNode)) {
    let (mut i, mut j) = (0, 0);
    loop {
        let p = match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) => a.piece.min(b.piece),
            (Some(a), None) => a.piece,
            (None, Some(b)) => b.piece,
            (None, None) => break,
        };
        let i2 = i + old[i..].iter().take_while(|x| x.piece == p).count();
        let j2 = j + new[j..].iter().take_while(|x| x.piece == p).count();
        let (a, b) = (&old[i..i2], &new[j..j2]);
        if a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x.flags != y.flags) {
            for x in a.iter().chain(b) { f(x); }
        }
        (i, j) = (i2, j2);
    }
}

impl Dirty {
    /// Whether a verdict of a crate whose dependency closure is `closure` and whose text writes
    /// `names` rests on anything dirty.
    fn reaches(&self, closure: &[u32], names: &[u64]) -> bool {
        closure.iter().any(|&c| self.coarse.get(c as usize).copied().unwrap_or(true)) || names.iter().any(|m| self.names.contains(m))
    }

    /// Node `keys`, `wide` and `named_head` dirty: the names it is found under, and for an impl
    /// that applies to types its self type does not name (a blanket impl, one for a primitive, a
    /// reference or an alias) its trait's name, its items' names, its trait's arguments' names
    /// and (`traits`: the trait definitions of those trait names are dirty) the trait's items'.
    fn mark(&mut self, keys: &[u64], wide: &[u64], named_head: bool, aliases: &HashSet<u64>, traits: &mut crate::fxhash::HashSet<u64>) -> bool {
        let mut grew = false;
        for &m in keys { grew |= self.names.insert(m); }
        if !wide.is_empty() && (!named_head || keys.iter().any(|k| aliases.contains(k))) {
            for &m in wide { grew |= self.names.insert(m); }
            grew |= traits.insert(wide[0]);
        }
        grew
    }

    /// What changed in the interfaces `changed` (each verdict's (crate, the interface it saw))
    /// since. The tables made now are kept (`Reuse::tables`: the check stores the missing ones).
    fn of(prog: &Program, r: &mut Reuse, store: &crate::store::Store, changed: &[(usize, Verdict, Vec<(u32, u128)>)], trace: bool) -> Dirty {
        let t0 = std::time::Instant::now();
        let n = prog.crates.len();
        let mut d = Dirty { unknown: HashSet::new(), coarse: vec![false; n], names: Default::default() };
        let user = crate::cache::user_names(prog);
        let mut pairs: Vec<(u32, u128)> = changed.iter().flat_map(|x| x.2.iter().copied()).filter(|p| p.0 != u32::MAX).collect();
        pairs.sort_unstable();
        pairs.dedup();
        let mut first: Vec<u32> = pairs.iter().map(|p| p.0).collect();
        first.dedup();
        let mut now: Vec<Option<Vec<CheckNode>>> = (0..n).map(|_| None).collect();
        for (c, v) in r.nodes_now(prog, &first, &user) {
            r.tables[c as usize] = Some(Table::of(r.heads[c as usize], &v));
            now[c as usize] = Some(v);
        }
        // the crates owning a dirty node; the traits whose impls for unnamed types are dirty
        let mut owns = vec![false; n];
        let mut traits: crate::fxhash::HashSet<u64> = Default::default();
        let mut changed_n = 0usize;
        for &(c, was) in &pairs {
            let old = match store.get_checked(table_key(r.idents[c as usize], was)) { Some((KIND_TABLE, p)) => decode_table(p), _ => None };
            let (Some(old), Some(new)) = (old, r.tables[c as usize].as_ref()) else { d.unknown.insert((c, was)); continue };
            if old.head != new.head { d.coarse[c as usize] = true; owns[c as usize] = true; continue; }
            let mut seen: Vec<&TNode> = Vec::new();
            changed_nodes(&old.nodes, &new.nodes, |x| seen.push(x));
            changed_n += seen.len();
            for x in seen {
                owns[c as usize] = true;
                if x.flags & NODE_ALWAYS != 0 { d.coarse[c as usize] = true; }
                d.mark(&x.keys, &x.wide, x.flags & NODE_NAMED_HEAD != 0, &user.aliases, &mut traits);
            }
        }
        let seeds = d.names.len();
        let mut passes = 0;
        if owns.iter().any(|o| *o) {
            // every node of every crate with an interface that names a dirty name is dirty, and so
            // are the names it is found under: whichever crate's (a check of a crate that sees
            // both finds an impl of one for a type the other names), to a fixpoint
            let rest: Vec<u32> = (0..n as u32).filter(|&c| r.ifaces[c as usize] != 0 && now[c as usize].is_none()).collect();
            for (c, v) in r.nodes_now(prog, &rest, &user) {
                if !r.table_stored[c as usize] { r.tables[c as usize] = Some(Table::of(r.heads[c as usize], &v)); }
                now[c as usize] = Some(v);
            }
            let mut done: Vec<Vec<bool>> = now.iter().map(|v| vec![false; v.as_ref().map_or(0, Vec::len)]).collect();
            let mut closures: HashMap<usize, Vec<u32>> = HashMap::new();
            loop {
                passes += 1;
                let mut grew = false;
                for c in 0..n {
                    let Some(v) = &now[c] else { continue };
                    // (a node invoking a macro it has no expansion of: dirty when a crate it can
                    // see owns a dirty node)
                    let mut sees: Option<bool> = None;
                    for (k, x) in v.iter().enumerate() {
                        if done[c][k] { continue; }
                        let hit = x.names.iter().any(|m| d.names.contains(m))
                            || (x.trait_def && x.keys.iter().any(|m| traits.contains(m)))
                            || (x.coarse && *sees.get_or_insert_with(|| owns[c] || closures.entry(c).or_insert_with(|| crate::check::dependency_closure(prog, c as u32)).iter().any(|&e| owns[e as usize])));
                        if !hit { continue; }
                        done[c][k] = true;
                        if !owns[c] { owns[c] = true; grew = true; }
                        if x.always && x.coarse { d.coarse[c] = true; }
                        grew |= d.mark(&x.keys, &x.wide, x.named_head, &user.aliases, &mut traits);
                    }
                }
                if !grew { break; }
            }
        }
        if trace || prog.session.options.verbose {
            let nodes: usize = now.iter().flatten().map(Vec::len).sum();
            let coarse: Vec<&str> = (0..n).filter(|&c| d.coarse[c]).map(|c| prog.syms.str(prog.crates[c].name)).collect();
            eprintln!("makepad check: changed interfaces: {} ({} unknown), {} changed nodes, {} seed names, {} dirty names after {} passes over {} nodes, coarse {:?}, {:.1} ms",
                pairs.len(), d.unknown.len(), changed_n, seeds, d.names.len(), passes, nodes, coarse, t0.elapsed().as_secs_f64() * 1e3);
        }
        d
    }
}

/// A check whose every crate is unchanged: its stored diagnostics `texts` (Reuse::reused_texts),
/// printed as the check that stored them printed them, and the summary of its `crates` checked
/// crates. Exit code 0 (the check that stored them passed).
pub(crate) fn print_reused_texts(texts: &[String], crates: usize) -> i32 {
    let entries: Vec<Entry> = texts.iter().map(|t| Entry { stage: Stage::new(Phase::Resolve, 0), seq: 0, out: Out::Warning(t.clone()), group_at: None }).collect();
    crate::check_order::print(&entries, true, true, crate::diag::enabled());
    eprintln!("makepad check: {} crates unchanged since a passing check: their verdicts reused", crates);
    eprintln!("makepad check: 0 body owners of the checked crates, 0 compiled in all, 0 errors");
    0
}

/// What a check's output rests on in its unit graph (unitgraph.rs Graph) beyond the load's
/// recipe: every crate as the graph has it (binary, test harness, package, its build script's
/// output), the root, the bins, the linked libraries.
pub(crate) fn graph_key(g: &crate::unitgraph::Graph) -> u128 {
    let mut h = Sip128::new();
    h.str("relax-check-graph");
    let strs = |h: &mut Sip128, v: &[String]| { h.u32(v.len() as u32); for s in v { h.str(s); } };
    let deps = |h: &mut Sip128, v: &[(usize, String)]| { h.u32(v.len() as u32); for (d, a) in v { h.u32(*d as u32); h.str(a); } };
    let env = |h: &mut Sip128, v: &[(String, String)]| { h.u32(v.len() as u32); for (k, x) in v { h.str(k); h.str(x); } };
    h.u32(g.crates.len() as u32);
    for (ci, c) in g.crates.iter().enumerate() {
        h.str(&c.name); h.str(&c.src_path); h.u32(c.edition as u32); strs(&mut h, &c.features); deps(&mut h, &c.deps);
        h.u8(c.proc_macro as u8); h.u8(c.bin as u8); h.u8(c.harness as u8); h.u8(c.build_script as u8); h.str(&c.version); h.u8(c.registry as u8);
        match &c.host {
            None => h.u8(0),
            Some(u) => {
                h.u8(1); strs(&mut h, &u.features); deps(&mut h, &u.deps);
                match &u.build_outputs { None => h.u8(0), Some((cfgs, vars)) => { h.u8(1); strs(&mut h, cfgs); env(&mut h, vars); } }
            }
        }
        let (dir, name, ver) = &g.pkg[ci];
        h.str(dir); h.str(name); h.str(ver);
        strs(&mut h, &g.cfgs[ci]);
        env(&mut h, &g.env[ci]);
    }
    h.u32(g.root as u32);
    h.u32(g.bins.len() as u32);
    for b in &g.bins { h.u32(*b as u32); }
    h.u32(g.link_libs.len() as u32);
    for (l, framework) in &g.link_libs { h.str(l); h.u8(*framework as u8); }
    h.u8(g.native.is_some() as u8);
    h.finish128()
}

/// A one-shot check's early record (run.rs): a check whose every crate hit and that printed its
/// stored diagnostics (`print_reused_texts`) stores what it printed, under a key known before the load:
/// the Relax build and options and the `MAKEPAD_RUN_*` variables (`env_key`), the command line,
/// the unit graph (`graph_key`), the load's recipe and the inputs and module probes of its front
/// graph certificate. The certificate is current only while every input has its recorded text
/// (cache.rs begin_load: a recorded stat stands for the text, else the file is hashed). A later
/// check with that key, whose certificate is current and whose late inputs (include_str!, env!)
/// are unchanged, prints the record right after the build scripts: no load, no verdict keys.
pub(crate) struct Early {
    path: PathBuf,
    key: u128,
    write: bool,
    /// the certificate's inputs: a record is stored only when the check's files are these
    inputs: HashMap<String, u128>,
    n_probes: usize,
}

/// A decoded early record.
struct EarlyRecord {
    crates: u32,
    late: Vec<(String, u8, String, u128)>,
    /// the stores the recording check used (store.rs live): kept in use by each replay
    stores: Vec<String>,
    texts: Vec<String>,
}

impl Early {
    /// The early record of a check (`args`: its command line) of the crates `pre` with the unit
    /// graph `graph` (`graph_key`). None: the cache is off or verifying, or the load's graph
    /// certificate is not current.
    pub(crate) fn open(prog: &Program, images: &crate::cache::FrontImages, pre: &[crate::preload::PreCrate], graph: u128, args: &[String]) -> Option<Early> {
        let mode = crate::cache::mode_from_env(&prog.session.options)?;
        if mode == crate::cache::Mode::Verify { return None; }
        let cert = images.certificate(pre)?;
        let mut h = Sip128::new();
        h.str("relax-check-early");
        h.u32(VERSION);
        h.u128(env_key(prog));
        h.u32(args.len() as u32);
        for a in args { h.str(a); }
        h.u128(graph);
        h.u128(cert.identity);
        h.u32(cert.inputs.len() as u32);
        for (p, x) in &cert.inputs { h.str(p); h.u128(*x); }
        h.u32(cert.probes.len() as u32);
        for (p, e) in &cert.probes { h.str(p); h.u8(*e as u8); }
        let path = crate::dirs::relax_dir().join("cache").join(format!("check-early-{:016x}.rcs", cert.identity as u64));
        Some(Early { path, key: h.finish128(), write: mode == crate::cache::Mode::On, inputs: cert.inputs.iter().cloned().collect(), n_probes: cert.probes.len() })
    }

    /// The stored output printed when the record is current (its late inputs unchanged): the
    /// check's exit code. None: no current record.
    pub(crate) fn replay(&self) -> Option<i32> {
        let times = std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some();
        let found = crate::store::Store::open(&self.path).and_then(|store| match store.get_checked(self.key) {
            Some((KIND_EARLY, p)) => decode_early(p),
            _ => None,
        });
        let current = found.filter(|r| r.late.iter().all(|(c, k, name, h)| late_current(c, *k, name, *h)));
        if times {
            eprintln!("makepad check: early record ({} inputs, {} probes): {}", self.inputs.len(), self.n_probes, if current.is_some() { "current" } else { "none" });
        }
        let r = current?;
        for s in &r.stores { crate::store::stamp_used(Path::new(s)); }
        Some(print_reused_texts(&r.texts, r.crates as usize))
    }

    /// Stores what a check whose every crate hit printed (`texts`, `reuse`'s): only when the
    /// check's files are the certificate's inputs as they were validated (a file the load read
    /// again could have changed since).
    pub(crate) fn record(&self, reuse: &Reuse, texts: &[String], verbose: bool) {
        if !self.write { return; }
        if let Some((p, _)) = reuse.files.iter().find(|(p, x)| self.inputs.get(p) != Some(x)) {
            if verbose { eprintln!("makepad check: early record not stored: {} is not the certificate's", p); }
            return;
        }
        let mut late = reuse.late.clone();
        late.extend(crate::sip::late_inputs());
        late.sort();
        late.dedup();
        let mut stores: Vec<String> = crate::store::live().into_iter().map(|p| p.display().to_string()).collect();
        stores.sort();
        let mut w = W(Vec::new());
        w.u32(VERSION);
        w.u32(reuse.crates() as u32);
        w.u32(late.len() as u32);
        for (k, kind, name, h) in &late { w.str(k); w.0.push(*kind); w.str(name); w.u128(*h); }
        w.u32(stores.len() as u32);
        for s in &stores { w.str(s); }
        w.u32(texts.len() as u32);
        for t in texts { w.str(t); }
        let entry = crate::pack::NewEntry { key: self.key, kind: KIND_EARLY, payload: w.0 };
        // (one record per recipe: the last check's)
        let r = self.path.parent().map_or(Ok(()), crate::pack::ensure_private_dir)
            .and_then(|_| crate::store::append(&self.path, vec![entry], true).map_err(|e| e.to_string()));
        if verbose { eprintln!("makepad check: early record stored ({:?})", r.err()); }
    }
}

fn decode_early(p: &[u8]) -> Option<EarlyRecord> {
    let mut r = R(p);
    if r.u32()? != VERSION { return None; }
    let crates = r.u32()?;
    let mut late = Vec::new();
    for _ in 0..r.u32()? { late.push((r.str()?, r.u8()?, r.str()?, r.u128()?)); }
    let mut stores = Vec::new();
    for _ in 0..r.u32()? { stores.push(r.str()?); }
    let mut texts = Vec::new();
    for _ in 0..r.u32()? { texts.push(r.str()?); }
    r.0.is_empty().then_some(EarlyRecord { crates, late, stores, texts })
}

struct W(Vec<u8>);
impl W {
    fn u32(&mut self, x: u32) { self.0.extend_from_slice(&x.to_le_bytes()); }
    fn u64(&mut self, x: u64) { self.0.extend_from_slice(&x.to_le_bytes()); }
    fn u128(&mut self, x: u128) { self.0.extend_from_slice(&x.to_le_bytes()); }
    fn str(&mut self, s: &str) { self.u32(s.len() as u32); self.0.extend_from_slice(s.as_bytes()); }
    fn u64s(&mut self, v: &[u64]) { self.u32(v.len() as u32); for x in v { self.u64(*x); } }
}

struct R<'a>(&'a [u8]);
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n { return None; }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn u64(&mut self) -> Option<u64> { Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?)) }
    fn u128(&mut self) -> Option<u128> { Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?)) }
    fn u64s(&mut self) -> Option<Vec<u64>> {
        let n = self.u32()? as usize;
        if self.0.len() < n.saturating_mul(8) { return None; }
        (0..n).map(|_| self.u64()).collect()
    }
    fn str(&mut self) -> Option<String> { let n = self.u32()? as usize; String::from_utf8(self.take(n)?.to_vec()).ok() }
}

fn encode(v: &Verdict) -> Vec<u8> {
    let mut w = W(Vec::new());
    w.u32(VERSION);
    w.u32(v.late.len() as u32);
    for (k, kind, name, h) in &v.late { w.str(k); w.0.push(*kind); w.str(name); w.u128(*h); }
    w.u32(v.refs.len() as u32);
    for (i, t) in &v.refs { w.u128(*i); w.u128(*t); }
    w.u32(v.deps.len() as u32);
    for (i, f) in &v.deps { w.u128(*i); w.u128(*f); }
    w.u64s(&v.names);
    w.u32(v.entries.len() as u32);
    for e in &v.entries {
        w.0.push(e.phase.code());
        w.0.push(e.step);
        w.0.push(e.flags);
        w.u32(e.seq);
        match &e.group {
            GroupAt::Zero => w.0.push(0),
            GroupAt::At => w.0.push(1),
            GroupAt::Path(p, l, col) => { w.0.push(2); w.str(p); w.u32(*l); w.u32(*col); }
        }
        w.str(&e.text);
    }
    w.0
}

fn encode_table(t: &Table) -> Vec<u8> {
    let mut w = W(Vec::new());
    w.u32(VERSION);
    w.u128(t.head);
    w.u32(t.nodes.len() as u32);
    for x in &t.nodes {
        w.u128(x.piece);
        w.0.push(x.flags);
        w.u64s(&x.keys);
        w.u64s(&x.wide);
    }
    w.0
}

fn decode_table(p: &[u8]) -> Option<Table> {
    let mut r = R(p);
    if r.u32()? != VERSION { return None; }
    let head = r.u128()?;
    let n = r.u32()? as usize;
    if r.0.len() < n.saturating_mul(25) { return None; }
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n { nodes.push(TNode { piece: r.u128()?, flags: r.u8()?, keys: r.u64s()?, wide: r.u64s()? }); }
    r.0.is_empty().then_some(Table { head, nodes })
}

fn decode(p: &[u8]) -> Option<Verdict> {
    let mut r = R(p);
    if r.u32()? != VERSION { return None; }
    let mut late = Vec::new();
    for _ in 0..r.u32()? { late.push((r.str()?, r.u8()?, r.str()?, r.u128()?)); }
    let mut refs = Vec::new();
    for _ in 0..r.u32()? { refs.push((r.u128()?, r.u128()?)); }
    let mut deps = Vec::new();
    for _ in 0..r.u32()? { deps.push((r.u128()?, r.u128()?)); }
    let names = r.u64s()?;
    let mut entries = Vec::new();
    for _ in 0..r.u32()? {
        let phase = Phase::of_code(r.u8()?)?;
        let (step, flags, seq) = (r.u8()?, r.u8()?, r.u32()?);
        let group = match r.u8()? {
            0 => GroupAt::Zero,
            1 => GroupAt::At,
            2 => GroupAt::Path(r.str()?, r.u32()?, r.u32()?),
            _ => return None,
        };
        entries.push(Stored { phase, step, flags, seq, group, text: r.str()? });
    }
    r.0.is_empty().then_some(Verdict { late, refs, deps, names, entries })
}

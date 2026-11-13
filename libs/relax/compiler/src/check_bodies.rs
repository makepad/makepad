//! Body verdicts: a fresh crate's body whose check's inputs are as a stored check saw them is
//! replayed, not checked -- its facts (relax_types body_facts.rs: what the crate passes after the
//! bodies read of it) recorded as that check left them.
//!
//! A body verdict's key is the crate's body base (check_reuse.rs `body_base`: the crate key's
//! parts without the crate's text, the crate's interface and its checked dependencies'
//! interfaces), the owner's stable identity (cache.rs `def_key_in`) and its body's text
//! (cache/items.rs `owner_body_hash`: a fn body no other item's check sees, with what its macros
//! expanded to). Only such a fn's body is stored, and only one whose check was clean and whose
//! facts hold no position (no lint reported, no expectation met, no dead-code note, nothing
//! accepted only in relaxed Rust): any other is checked every time. A check that met a
//! constant's evaluation failure, a query error or a delayed bug stores none.
//!
//! A crate's verdicts are one blob in the check store (`KIND_BODIES`, keyed by the body base):
//! the late inputs its check read (any value changed, none of them stands), a stable table, and
//! each body's facts by key. The table names definitions, modules, crates and symbols: their
//! process-local indices cannot survive another process or another load order. Resolution
//! computes def_key_in only for matching name-and-kind candidates (about 1.6 us per def,
//! measured), never for a crate's whole definition list.
//!
//! A replayed body is still parsed and its macros expanded (check.rs): the late inputs it reads,
//! the names its expansions write and the expansion records the early lints read come from that.
//! Verify mode (cache Mode::Verify) checks every body and compares the stored verdicts under each
//! body's key with its check's facts.

use crate::cache::{def_key_in, def_kind_code};
use crate::program::{DefId, DefKind, Program, Sym};
use crate::tcx::Tcx;
use relax_types::body_facts::{BodyFacts, FactCodec, FactDecodec};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::cell::Cell;

/// A definition's stable identity, with name and kind narrowing the expensive key comparisons.
#[derive(Clone)]
struct DefEntry {
    /// The defining crate's identity, independent of the order crates were loaded.
    krate: u128,
    /// The spelling used to find candidates without interning new symbols.
    name: String,
    /// The cache's stable kind tag, so equally named items of other kinds are not candidates.
    kind: u32,
    /// The cache's definition key, distinguishing candidates within the crate.
    key: u128,
}

/// The encoder's stable names and deduplication maps, checked by one resolver before use.
pub(crate) struct FactTable<'a> {
    /// The checked program supplies names and module ownership.
    prog: &'a Program,
    /// Collected types provide the impl identities used by definition keys.
    tcx: &'a Tcx,
    /// Crate identities in this program's crate order, shared by all body encodings.
    idents: &'a [u128],
    /// Stable definitions in the order their indices were assigned.
    defs: Vec<DefEntry>,
    /// Module definitions by table index, so load-local module numbers are never written.
    modules: Vec<u32>,
    /// Crate identities in the order their indices were assigned.
    crates: Vec<u128>,
    /// Symbol text in the order its indices were assigned.
    syms: Vec<String>,
    /// Definition indices or failed stable names, so a failure is not recomputed per reference.
    def_indices: HashMap<DefId, Option<u32>>,
    /// Module indices or failed ownership checks, including block modules.
    module_indices: HashMap<u32, Option<u32>>,
    /// Crate indices already assigned to identities.
    crate_indices: HashMap<u128, u32>,
    /// Symbol indices already assigned to text.
    sym_indices: HashMap<String, u32>,
    /// A stored table's definitions by identity (`extend`), used again where they resolve to the
    /// definition named.
    def_at: HashMap<(u128, String, u32, u128), u32>,
    /// A stored table's modules by their definition's index (`extend`).
    module_at: HashMap<u32, u32>,
    /// A resolver fed each new entry, proving that it names exactly the encoder's value.
    resolver: FactResolver<'a>,
}

/// Builds a single table for a check so bodies share stable names and resolution work.
impl<'a> FactTable<'a> {
    /// Starts empty while preparing the resolver's crate lookup and one definition bucketing pass.
    pub(crate) fn new(prog: &'a Program, tcx: &'a Tcx, idents: &'a [u128]) -> FactTable<'a> {
        FactTable {
            prog, tcx, idents,
            defs: Vec::new(), modules: Vec::new(), crates: Vec::new(), syms: Vec::new(),
            def_indices: HashMap::new(), module_indices: HashMap::new(),
            crate_indices: HashMap::new(), sym_indices: HashMap::new(),
            def_at: HashMap::new(), module_at: HashMap::new(),
            resolver: FactResolver::new(prog, tcx, idents),
        }
    }

    /// The table of a stored blob (`resolver`), its entries kept at their indices (the facts of
    /// the bodies replayed from it name them) and new ones added after them; an entry already
    /// there is used again when it resolves to the value named.
    pub(crate) fn extend(resolver: FactResolver<'a>) -> FactTable<'a> {
        let def_at = resolver.defs.iter().enumerate().rev().map(|(i, e)| ((e.krate, e.name.clone(), e.kind, e.key), i as u32)).collect();
        let module_at = resolver.modules.iter().enumerate().rev().map(|(i, &d)| (d, i as u32)).collect();
        let crate_indices = resolver.crates.iter().enumerate().rev().map(|(i, &k)| (k, i as u32)).collect();
        let sym_indices = resolver.syms.iter().enumerate().rev().map(|(i, s)| (s.clone(), i as u32)).collect();
        FactTable {
            prog: resolver.prog, tcx: resolver.tcx, idents: resolver.idents,
            defs: resolver.defs.clone(), modules: resolver.modules.clone(), crates: resolver.crates.clone(), syms: resolver.syms.clone(),
            def_indices: HashMap::new(), module_indices: HashMap::new(), crate_indices, sym_indices, def_at, module_at, resolver,
        }
    }

    /// Writes the stable table in index order so a new resolver needs no encoder-side state.
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        let mut w = W(out);
        w.u32(self.defs.len() as u32);
        for e in &self.defs { w.u128(e.krate); w.str(&e.name); w.u32(e.kind); w.u128(e.key); }
        w.u32(self.modules.len() as u32);
        for &m in &self.modules { w.u32(m); }
        w.u32(self.crates.len() as u32);
        for &k in &self.crates { w.u128(k); }
        w.u32(self.syms.len() as u32);
        for s in &self.syms { w.str(s); }
    }
}

/// Rejects definitions and modules whose stable names cannot find the original value uniquely.
impl FactCodec for FactTable<'_> {
    /// Adds a definition only when the resolver finds exactly this DefId, remembering failures too.
    fn def(&mut self, d: DefId) -> Option<u32> {
        if let Some(&got) = self.def_indices.get(&d) { return got; }
        let got = (|| {
            if d.0 as usize >= self.prog.defs.len() { return None; }
            let def = self.prog.def(d);
            let e = DefEntry {
                krate: *self.idents.get(def.krate as usize)?,
                name: self.prog.syms.str(def.name).to_string(),
                kind: def_kind_code(&def.kind),
                // (the resolver's memos: its candidate keys for this entry include this one)
                key: def_key_in(self.prog, self.tcx, self.idents, &mut self.resolver.memo, &mut self.resolver.files, d),
            };
            if let Some(&i) = self.def_at.get(&(e.krate, e.name.clone(), e.kind, e.key)) {
                if self.resolver.def(i) == Some(d) { return Some(i); }
            }
            let i = u32::try_from(self.defs.len()).ok()?;
            self.resolver.defs.push(e.clone());
            self.resolver.resolved_defs.push(None);
            if self.resolver.def(i) != Some(d) {
                self.resolver.defs.pop();
                self.resolver.resolved_defs.pop();
                return None;
            }
            self.defs.push(e);
            Some(i)
        })();
        self.def_indices.insert(d, got);
        got
    }

    /// Names a module by its own definition; a block borrowing another module's def is rejected.
    fn module(&mut self, m: u32) -> Option<u32> {
        if let Some(&got) = self.module_indices.get(&m) { return got; }
        let got = (|| {
            let def = self.prog.mods.get(m as usize)?.def;
            let def = self.def(def)?;
            if let Some(&i) = self.module_at.get(&def) {
                if self.resolver.module(i) == Some(m) { return Some(i); }
            }
            let i = u32::try_from(self.modules.len()).ok()?;
            self.resolver.modules.push(def);
            self.resolver.resolved_modules.push(None);
            if self.resolver.module(i) != Some(m) {
                self.resolver.modules.pop();
                self.resolver.resolved_modules.pop();
                return None;
            }
            self.modules.push(def);
            Some(i)
        })();
        self.module_indices.insert(m, got);
        got
    }

    /// Deduplicates crate identities so a later load can assign different crate numbers.
    fn krate(&mut self, k: u32) -> Option<u32> {
        let ident = *self.idents.get(k as usize)?;
        if let Some(&i) = self.crate_indices.get(&ident) { return Some(i); }
        let i = u32::try_from(self.crates.len()).ok()?;
        self.crates.push(ident);
        self.resolver.crates.push(ident);
        self.resolver.resolved_crates.push(None);
        self.crate_indices.insert(ident, i);
        Some(i)
    }

    /// Deduplicates symbol text so no interner indices enter stored facts.
    fn sym(&mut self, s: Sym) -> Option<u32> {
        let name = self.prog.syms.str(s);
        if let Some(&i) = self.sym_indices.get(name) { return Some(i); }
        let i = u32::try_from(self.syms.len()).ok()?;
        self.syms.push(name.to_string());
        self.resolver.syms.push(name.to_string());
        self.resolver.resolved_syms.push(None);
        self.sym_indices.insert(name.to_string(), i);
        Some(i)
    }
}

/// A table in the reading program, resolving only requested entries and memoizing misses too.
pub(crate) struct FactResolver<'a> {
    /// The reading program supplies the current DefIds, modules and symbols.
    prog: &'a Program,
    /// Collected impl types participate in stable definition keys.
    tcx: &'a Tcx,
    /// Current crate identities let candidate keys use the same root as stored keys.
    idents: &'a [u128],
    /// Definition keys shared by all candidates and entries resolved in this table.
    memo: HashMap<DefId, u128>,
    /// File keys shared by candidate key computations.
    files: HashMap<u32, u128>,
    /// Stored definition names and keys in table order.
    defs: Vec<DefEntry>,
    /// Stored module-definition indices in table order.
    modules: Vec<u32>,
    /// Stored crate identities in table order.
    crates: Vec<u128>,
    /// Stored symbol text in table order.
    syms: Vec<String>,
    /// Identity lookup built once, independent of the reading program's crate order.
    crate_ids: HashMap<u128, u32>,
    /// One pass buckets every definition by crate without computing any stable keys.
    crate_defs: Vec<Vec<DefId>>,
    /// A crate's name-and-kind buckets, built only when its first definition is requested.
    candidates: Vec<Option<HashMap<(Sym, u32), Vec<DefId>>>>,
    /// None means unqueried; Some(None) keeps an absent or ambiguous definition from being retried.
    resolved_defs: Vec<Option<Option<DefId>>>,
    /// Cached current module numbers, including failed module-definition checks.
    resolved_modules: Vec<Option<Option<u32>>>,
    /// Cached current crate numbers, including identities absent from this program.
    resolved_crates: Vec<Option<Option<u32>>>,
    /// Cached existing symbols, including names absent from this program's interner.
    resolved_syms: Vec<Option<Option<Sym>>>,
}

/// Reconstructs table state without carrying any memo from the encoding program.
impl<'a> FactResolver<'a> {
    /// Prepares crate lookup and definition buckets once, leaving stable keys and name indices lazy.
    fn new(prog: &'a Program, tcx: &'a Tcx, idents: &'a [u128]) -> FactResolver<'a> {
        let mut crate_defs = vec![Vec::new(); prog.crates.len()];
        for (i, def) in prog.defs.iter().enumerate() { crate_defs[def.krate as usize].push(DefId(i as u32)); }
        FactResolver {
            prog, tcx, idents, memo: HashMap::new(), files: HashMap::new(),
            defs: Vec::new(), modules: Vec::new(), crates: Vec::new(), syms: Vec::new(),
            crate_ids: idents.iter().enumerate().map(|(i, &k)| (k, i as u32)).collect(),
            crate_defs, candidates: vec![None; prog.crates.len()],
            resolved_defs: Vec::new(), resolved_modules: Vec::new(),
            resolved_crates: Vec::new(), resolved_syms: Vec::new(),
        }
    }

    /// Reads one table prefix, leaving body bytes to their own codec and rejecting malformed entries.
    pub(crate) fn read(prog: &'a Program, tcx: &'a Tcx, idents: &'a [u128], bytes: &mut &[u8]) -> Option<FactResolver<'a>> {
        if idents.len() != prog.crates.len() { return None; }
        let mut r = R(*bytes);
        let mut resolver = FactResolver::new(prog, tcx, idents);
        for _ in 0..r.u32()? {
            resolver.defs.push(DefEntry { krate: r.u128()?, name: r.str()?, kind: r.u32()?, key: r.u128()? });
        }
        for _ in 0..r.u32()? {
            let def = r.u32()?;
            if def as usize >= resolver.defs.len() { return None; }
            resolver.modules.push(def);
        }
        for _ in 0..r.u32()? { resolver.crates.push(r.u128()?); }
        for _ in 0..r.u32()? { resolver.syms.push(r.str()?); }
        resolver.resolved_defs.resize(resolver.defs.len(), None);
        resolver.resolved_modules.resize(resolver.modules.len(), None);
        resolver.resolved_crates.resize(resolver.crates.len(), None);
        resolver.resolved_syms.resize(resolver.syms.len(), None);
        *bytes = r.0;
        Some(resolver)
    }
}

/// Maps stored indices to current values without interning names or accepting ambiguous keys.
impl FactDecodec for FactResolver<'_> {
    /// Computes keys only for matching name-and-kind candidates and requires exactly one match.
    fn def(&mut self, i: u32) -> Option<DefId> {
        let i = i as usize;
        if let Some(got) = *self.resolved_defs.get(i)? { return got; }
        let got = (|| {
            let entry = &self.defs[i];
            let krate = *self.crate_ids.get(&entry.krate)? as usize;
            let name = self.prog.syms.get(&entry.name)?;
            if self.candidates[krate].is_none() {
                let mut by_name: HashMap<(Sym, u32), Vec<DefId>> = HashMap::new();
                for &d in &self.crate_defs[krate] {
                    let def = self.prog.def(d);
                    by_name.entry((def.name, def_kind_code(&def.kind))).or_default().push(d);
                }
                self.candidates[krate] = Some(by_name);
            }
            let candidates = self.candidates[krate].as_ref()?.get(&(name, entry.kind))?;
            let mut found = None;
            for &d in candidates {
                if def_key_in(self.prog, self.tcx, self.idents, &mut self.memo, &mut self.files, d) == entry.key {
                    if found.is_some() { return None; }
                    found = Some(d);
                }
            }
            found
        })();
        self.resolved_defs[i] = Some(got);
        got
    }

    /// Uses a module definition's documented sub index and verifies that it owns that module.
    fn module(&mut self, i: u32) -> Option<u32> {
        let i = i as usize;
        if let Some(got) = *self.resolved_modules.get(i)? { return got; }
        let got = (|| {
            let d = self.def(self.modules[i])?;
            let def = self.prog.def(d);
            if def.kind != DefKind::Mod { return None; }
            (self.prog.mods.get(def.sub as usize)?.def == d).then_some(def.sub)
        })();
        self.resolved_modules[i] = Some(got);
        got
    }

    /// Resolves a crate identity once so reordered loads do not change the fact's meaning.
    fn krate(&mut self, i: u32) -> Option<u32> {
        let i = i as usize;
        if let Some(got) = *self.resolved_crates.get(i)? { return got; }
        let got = self.crate_ids.get(&self.crates[i]).copied();
        self.resolved_crates[i] = Some(got);
        got
    }

    /// Finds existing symbol text without mutating the reading program's symbol table.
    fn sym(&mut self, i: u32) -> Option<Sym> {
        let i = i as usize;
        if let Some(got) = *self.resolved_syms.get(i)? { return got; }
        let got = self.prog.syms.get(&self.syms[i]);
        self.resolved_syms[i] = Some(got);
        got
    }
}

/// The blob format's version: a blob of another is no blob of this check's (it is in the key).
pub(crate) const BLOB_VERSION: u32 = 1;

/// One fresh crate's stored body verdicts as this check read them.
struct Blob {
    /// The crate.
    krate: u32,
    /// Its body base: the blob's store key.
    key: u128,
    /// The payload when it stands (its late inputs unchanged), else empty.
    raw: Vec<u8>,
    /// The table's range in `raw`.
    table: (usize, usize),
    /// Each stored body's facts: body key -> range in `raw`.
    entries: HashMap<u128, (usize, usize)>,
}

/// What `Bodies` counted.
#[derive(Default)]
pub(crate) struct Stats {
    /// The owners whose verdicts can be stored.
    pub(crate) owners: usize,
    /// The owners replayed from stored verdicts.
    pub(crate) replayed: usize,
    /// Verify: the stored verdicts compared, and those that differ.
    pub(crate) verified: usize,
    pub(crate) differ: usize,
    /// The verdicts this check stored anew.
    pub(crate) stored: usize,
    /// The replayed owners checked after all (lint_imports::Judge), and those whose stored
    /// verdict differed from what their check found.
    pub(crate) rechecked: usize,
    pub(crate) recheck_differ: usize,
    /// Where the time went: keys, blob reads and decoding before the bodies; encoding after.
    pub(crate) open: std::time::Duration,
    pub(crate) store: std::time::Duration,
}

/// One check's body verdicts (module doc): the replayed owners, and after the bodies the facts
/// to store.
pub(crate) struct Bodies {
    /// Verify mode: compare, never replay.
    verify: bool,
    /// The check stores what it finds.
    write: bool,
    /// By fresh crate.
    blobs: Vec<Blob>,
    /// The owners whose verdicts can be stored: owner -> (blob, body key).
    owners: HashMap<DefId, (usize, u128)>,
    /// The owners replayed, and in Verify mode the stored facts of the owners with a verdict.
    replayed: HashSet<DefId>,
    expected: HashMap<DefId, BodyFacts>,
    /// The fresh owners' facts to store (`collect`), canonical.
    fresh: Vec<(DefId, BodyFacts)>,
    /// By crate: checked (a stored verdict keeps the import uses in their modules).
    checked: Vec<bool>,
    pub(crate) stats: Stats,
}

impl Bodies {
    /// Before the bodies: each fresh crate's blob, each storable owner's key among `fns`, and the
    /// stored facts of the owners whose verdict stands (to replay: none in Verify mode).
    pub(crate) fn open(reuse: &mut crate::check_reuse::Reuse, prog: &Program, tcx: &Tcx, fresh: &[u32], fns: &[(crate::borrowck::body::CheckedBody, bool)]) -> (Bodies, Vec<(DefId, BodyFacts)>) {
        let t0 = std::time::Instant::now();
        let mut b = Bodies {
            verify: reuse.verifying(), write: reuse.writes(), blobs: Vec::new(), owners: HashMap::new(),
            replayed: HashSet::new(), expected: HashMap::new(), fresh: Vec::new(), checked: vec![false; prog.crates.len()],
            stats: Stats::default(),
        };
        for &c in reuse.checked() { b.checked[c as usize] = true; }
        let mut slot: Vec<Option<usize>> = vec![None; prog.crates.len()];
        for &c in fresh {
            let Some(key) = reuse.body_base(prog, c) else { continue };
            slot[c as usize] = Some(b.blobs.len());
            b.blobs.push(Blob { krate: c, key, raw: Vec::new(), table: (0, 0), entries: HashMap::new() });
        }
        let idents = reuse.idents().to_vec();
        let (mut memo, mut files) = (HashMap::new(), HashMap::new());
        // (owner, blob, owner key, body key)
        let mut keyed: Vec<(DefId, usize, u128, u128)> = Vec::new();
        for (owner, _) in fns {
            if owner.kind != crate::borrowck::body::BodyKind::Function { continue; }
            let d = owner.owner;
            let def = prog.def(d);
            let Some(bi) = slot.get(def.krate as usize).copied().flatten() else { continue };
            let (Some(pf), Some(dg)) = (prog.files.get(def.file as usize), reuse.digest(def.file as usize)) else { continue };
            let Some(text) = crate::cache::owner_body_hash(pf, dg, def.item) else { continue };
            let owner_key = def_key_in(prog, tcx, &idents, &mut memo, &mut files, d);
            let mut h = crate::sip::Sip128::new();
            h.str("relax-check-body");
            h.u128(b.blobs[bi].key);
            h.u128(owner_key);
            h.u128(text);
            keyed.push((d, bi, owner_key, h.finish128()));
        }
        // two owners of one stable identity: neither is stored (a verdict names no other's facts)
        let mut seen: HashMap<(usize, u128), usize> = HashMap::new();
        for k in &keyed { *seen.entry((k.1, k.2)).or_default() += 1; }
        b.owners = keyed.into_iter().filter(|k| seen[&(k.1, k.2)] == 1).map(|(d, bi, _, key)| (d, (bi, key))).collect();
        b.stats.owners = b.owners.len();
        // the blobs whose late inputs stand
        let mut late_memo: HashMap<(String, u8, String), u128> = HashMap::new();
        let keys: Vec<u128> = b.blobs.iter().map(|x| x.key).collect();
        for (blob, raw) in b.blobs.iter_mut().zip(reuse.body_blobs(&keys)) {
            let Some(raw) = raw else { continue };
            let Some(parsed) = parse_blob(&raw) else { continue };
            let stands = parsed.late.iter().all(|(c, kind, name, hash)| {
                *late_memo.entry((c.clone(), *kind, name.clone())).or_insert_with(|| crate::check_reuse::late_now(c, *kind, name)) == *hash
            });
            if !stands { continue; }
            blob.table = parsed.table;
            blob.entries = parsed.entries;
            blob.raw = raw;
        }
        // the owners whose verdicts stand: decoded with their blob's table
        let mut by_blob: Vec<Vec<(DefId, u128)>> = vec![Vec::new(); b.blobs.len()];
        for (&d, &(bi, key)) in &b.owners { if b.blobs[bi].entries.contains_key(&key) { by_blob[bi].push((d, key)); } }
        let mut replay = Vec::new();
        for (blob, mut want) in b.blobs.iter().zip(by_blob) {
            if want.is_empty() { continue; }
            want.sort_unstable();
            let mut table = &blob.raw[blob.table.0..blob.table.1];
            let Some(mut resolver) = FactResolver::read(prog, tcx, &idents, &mut table) else { continue };
            for (d, key) in want {
                let (lo, hi) = blob.entries[&key];
                let Some(facts) = BodyFacts::decode(&blob.raw[lo..hi], &mut resolver) else { continue };
                if b.verify { b.expected.insert(d, facts); continue; }
                #[cfg(test)]
                let facts = test_dropped_uses(prog, d, facts);
                b.replayed.insert(d);
                replay.push((d, facts));
            }
        }
        b.stats.replayed = b.replayed.len();
        b.stats.open = t0.elapsed();
        (b, replay)
    }

    /// The owners replayed.
    pub(crate) fn replayed(&self) -> &HashSet<DefId> { &self.replayed }

    /// The replayed owners `again` checked after all (lint_imports::Judge: a crate's import
    /// reports wait on them), `facts` their facts (`unclean`: the failed ones), taken out of the
    /// replayed: each one's stored verdict against its check, the differences described (a
    /// verdict that lacked a use is how a report would have been false), and its facts stored
    /// anew, as a checked owner's.
    pub(crate) fn rechecked(&mut self, prog: &Program, tcx: &Tcx, idents: &[u128], facts: &[(DefId, BodyFacts)], unclean: &HashSet<DefId>, again: &[DefId]) -> Vec<String> {
        let mut out = Vec::new();
        let none = BodyFacts::default();
        let mut by_blob: HashMap<usize, Vec<DefId>> = HashMap::new();
        for &d in again {
            if self.replayed.remove(&d) { by_blob.entry(self.owners[&d].0).or_default().push(d); }
        }
        for (bi, mut ds) in by_blob {
            ds.sort_unstable();
            let blob = &self.blobs[bi];
            let mut table = &blob.raw[blob.table.0..blob.table.1];
            let mut resolver = FactResolver::read(prog, tcx, idents, &mut table);
            for d in ds {
                self.stats.rechecked += 1;
                let (lo, hi) = blob.entries[&self.owners[&d].1];
                let stored = resolver.as_mut().and_then(|r| BodyFacts::decode(&blob.raw[lo..hi], r));
                let fresh = if unclean.contains(&d) { None } else {
                    let f = facts.binary_search_by_key(&d, |x| x.0).map_or(&none, |i| &facts[i].1);
                    f.stored(prog, prog.def(d).krate).map(|mut f| { f.retain_uses(prog, |c| self.checked[c as usize]); f.canonical() })
                };
                let why = match (&stored, &fresh) {
                    (Some(s), Some(f)) => s.canonical().first_difference(prog, f),
                    (_, None) => Some("the body's check failed or left facts with positions".to_string()),
                    (None, Some(_)) => Some("its stored verdict reads no more".to_string()),
                };
                if let Some(why) = why {
                    self.stats.recheck_differ += 1;
                    out.push(format!("`{}`: {}", prog.def_path(d), why));
                }
                if let Some(f) = fresh.filter(|_| self.write && !self.verify) { self.fresh.push((d, f)); }
            }
        }
        out
    }

    /// Verify mode: the stored verdicts are compared, none replayed.
    pub(crate) fn verifying(&self) -> bool { self.verify }

    /// After the bodies (and their anonymous constants), before the facts are published: the
    /// facts of each fresh owner whose check was clean (`unclean`: the others) and that hold no
    /// position, to store. Verify mode: each against its stored verdict; the differences, as the
    /// check's internal errors.
    pub(crate) fn collect(&mut self, prog: &Program, facts: &[(DefId, BodyFacts)], unclean: &HashSet<DefId>) -> Vec<String> {
        let mut out = Vec::new();
        let mut owners: Vec<DefId> = self.owners.keys().copied().filter(|d| !self.replayed.contains(d)).collect();
        owners.sort_unstable();
        let none = BodyFacts::default();
        let checked = &self.checked;
        for d in owners {
            let failed = unclean.contains(&d);
            let stored = if failed { None } else {
                let f = facts.binary_search_by_key(&d, |x| x.0).map_or(&none, |i| &facts[i].1);
                f.stored(prog, prog.def(d).krate).map(|mut f| { f.retain_uses(prog, |c| checked[c as usize]); f.canonical() })
            };
            if let Some(expected) = self.expected.remove(&d) {
                self.stats.verified += 1;
                let why = match &stored {
                    None if failed => Some("the body's check failed or met a file the load skipped".to_string()),
                    None => Some("the body's check left facts with positions (a lint, an expectation, a note)".to_string()),
                    Some(f) => expected.canonical().first_difference(prog, f),
                };
                if let Some(why) = why {
                    self.stats.differ += 1;
                    out.push(format!("internal: a stored body verdict differs from the body's check: `{}`: {}", prog.def_path(d), why));
                }
            }
            if let Some(f) = stored.filter(|_| self.write && !self.verify) { self.fresh.push((d, f)); }
        }
        out
    }

    /// At the end of the check: the blob of each fresh crate with a body checked and stored anew,
    /// its replayed bodies' facts as they were (`blocked`: the check met a failure no body is
    /// charged with -- nothing stored), handed to `reuse` to store. What was counted.
    pub(crate) fn store(mut self, reuse: &mut crate::check_reuse::Reuse, prog: &Program, tcx: &Tcx, blocked: bool) -> Stats {
        if !self.write || self.verify { return self.stats; }
        let t0 = std::time::Instant::now();
        let idents = reuse.idents().to_vec();
        let mut fresh: Vec<Vec<(DefId, BodyFacts)>> = (0..self.blobs.len()).map(|_| Vec::new()).collect();
        if !blocked {
            for (d, f) in std::mem::take(&mut self.fresh) { fresh[self.owners[&d].0].push((d, f)); }
        }
        let mut replayed: Vec<Vec<u128>> = vec![Vec::new(); self.blobs.len()];
        for d in &self.replayed { let (bi, key) = self.owners[d]; replayed[bi].push(key); }
        let (mut new, mut keep) = (Vec::new(), Vec::new());
        for (bi, blob) in self.blobs.iter().enumerate() {
            if fresh[bi].is_empty() {
                if !blob.raw.is_empty() { keep.push((blob.key, blob.raw.clone())); }
                continue;
            }
            // the stored table, extended, while its bodies are replayed (their facts name its
            // indices: carried only with it); else a new one
            let extended = if replayed[bi].is_empty() { None } else {
                let mut t = &blob.raw[blob.table.0..blob.table.1];
                FactResolver::read(prog, tcx, &idents, &mut t).map(FactTable::extend)
            };
            let carried: &[u128] = if extended.is_some() { &replayed[bi] } else { &[] };
            let mut table = extended.unwrap_or_else(|| FactTable::new(prog, tcx, &idents));
            let mut encoded: Vec<(u128, Vec<u8>)> = Vec::new();
            for (d, f) in &fresh[bi] {
                let mut bytes = Vec::new();
                if f.encode(&mut table, &mut bytes) { encoded.push((self.owners[d].1, bytes)); }
            }
            self.stats.stored += encoded.len();
            let mut entries: Vec<(u128, &[u8])> = carried.iter().map(|key| { let (lo, hi) = blob.entries[key]; (*key, &blob.raw[lo..hi]) }).collect();
            entries.extend(encoded.iter().map(|(k, v)| (*k, v.as_slice())));
            entries.sort_unstable_by_key(|e| e.0);
            entries.dedup_by_key(|e| e.0);
            new.push((blob.key, write_blob(&reuse.late_inputs_of(prog, blob.krate), &table, &entries)));
        }
        reuse.put_bodies(new, keep);
        self.stats.store = t0.elapsed();
        self.stats
    }
}

/// A test's stand-in for a stored verdict that lacks a use (check_reuse_tests): the stored
/// facts of the owner whose path ends with RELAX_CHECK_REUSE_DROP_USES hold no import uses.
#[cfg(test)]
fn test_dropped_uses(prog: &Program, d: DefId, mut facts: BodyFacts) -> BodyFacts {
    if std::env::var("RELAX_CHECK_REUSE_DROP_USES").is_ok_and(|n| prog.def_path(d).ends_with(n.as_str())) {
        facts.import_uses = Default::default();
    }
    facts
}

/// A blob: the version, the late inputs (crate, kind, name, value), the table (its length first),
/// the bodies by key (key, length, facts).
fn write_blob(late: &[(String, u8, String, u128)], table: &FactTable, entries: &[(u128, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut tb = Vec::new();
    table.write(&mut tb);
    let mut w = W(&mut out);
    w.u32(BLOB_VERSION);
    w.u32(late.len() as u32);
    for (c, kind, name, hash) in late { w.str(c); w.u8(*kind); w.str(name); w.u128(*hash); }
    w.u32(tb.len() as u32);
    w.0.extend_from_slice(&tb);
    w.u32(entries.len() as u32);
    for (key, bytes) in entries { w.u128(*key); w.u32(bytes.len() as u32); w.0.extend_from_slice(bytes); }
    out
}

/// A blob read (`write_blob`): its late inputs, and the table's and each body's range.
struct Parsed {
    late: Vec<(String, u8, String, u128)>,
    table: (usize, usize),
    entries: HashMap<u128, (usize, usize)>,
}

fn parse_blob(p: &[u8]) -> Option<Parsed> {
    let mut r = R(p);
    let at = |r: &R| p.len() - r.0.len();
    if r.u32()? != BLOB_VERSION { return None; }
    let mut late = Vec::new();
    for _ in 0..r.u32()? { late.push((r.str()?, r.u8()?, r.str()?, r.u128()?)); }
    let n = r.u32()? as usize;
    let lo = at(&r);
    r.take(n)?;
    let table = (lo, lo + n);
    let mut entries = HashMap::new();
    for _ in 0..r.u32()? {
        let key = r.u128()?;
        let n = r.u32()? as usize;
        let lo = at(&r);
        r.take(n)?;
        entries.insert(key, (lo, lo + n));
    }
    r.0.is_empty().then_some(Parsed { late, table, entries })
}

/// Little-endian table output, following check_reuse's length-prefixed encoding.
struct W<'a>(&'a mut Vec<u8>);

/// Keeps the table's scalar and string encoding together so the reader mirrors it.
impl W<'_> {
    /// Writes a late input's kind.
    fn u8(&mut self, x: u8) { self.0.push(x); }
    /// Writes a table count or index in a process-independent byte order.
    fn u32(&mut self, x: u32) { self.0.extend_from_slice(&x.to_le_bytes()); }
    /// Writes an identity or key without depending on machine endianness.
    fn u128(&mut self, x: u128) { self.0.extend_from_slice(&x.to_le_bytes()); }
    /// Prefixes UTF-8 text with its byte length so adjacent names remain distinct.
    fn str(&mut self, s: &str) { self.u32(s.len() as u32); self.0.extend_from_slice(s.as_bytes()); }
}

/// A table-prefix reader whose remaining slice preserves the body bytes following the table.
struct R<'a>(&'a [u8]);

/// Checks byte availability before every scalar or text read, avoiding limits unrelated to the input.
impl<'a> R<'a> {
    /// Takes an available prefix, rejecting truncation before splitting the input.
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n { return None; }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    /// Reads a late input's kind.
    fn u8(&mut self) -> Option<u8> { Some(self.take(1)?[0]) }
    /// Reads a count or index in the writer's little-endian order.
    fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    /// Reads a complete crate identity or definition key.
    fn u128(&mut self) -> Option<u128> { Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?)) }
    /// Rejects truncated or invalid UTF-8 names instead of inventing a replacement symbol.
    fn str(&mut self) -> Option<String> { let n = self.u32()? as usize; String::from_utf8(self.take(n)?.to_vec()).ok() }
}

#[cfg(test)]
thread_local! {
    /// Opts one check on the test thread into the codec proof without changing ordinary checks.
    pub(crate) static FACT_CODEC_CHECK: Cell<bool> = const { Cell::new(false) };
    /// The checked owners the proof found unstorable (facts with positions), by path.
    pub(crate) static FACT_CODEC_REFUSED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Proves stored facts keep their meaning with a fresh resolver, before publishing the original facts.
#[cfg(test)]
pub(crate) fn assert_round_trip(prog: &Program, tcx: &Tcx, own_all: &[bool], facts: &[(DefId, relax_types::body_facts::BodyFacts)]) {
    use relax_types::body_facts::BodyFacts;
    let idents: Vec<_> = (0..prog.crates.len()).map(|k| crate::check_reuse::ident(prog, k as u32)).collect();
    let mut table = FactTable::new(prog, tcx, &idents);
    let mut encoded = Vec::new();
    let mut failed = 0;
    for (owner, facts) in facts {
        let krate = prog.def(*owner).krate;
        if !own_all[krate as usize] { continue; }
        let Some(stored) = facts.stored(prog, krate) else {
            FACT_CODEC_REFUSED.with(|r| r.borrow_mut().push(prog.def_path(*owner)));
            continue;
        };
        let mut bytes = Vec::new();
        if stored.encode(&mut table, &mut bytes) { encoded.push((*owner, stored, bytes)); } else { failed += 1; }
    }
    let mut bytes = Vec::new();
    table.write(&mut bytes);
    let mut bytes = bytes.as_slice();
    let mut resolver = FactResolver::read(prog, tcx, &idents, &mut bytes).expect("body-fact table did not decode");
    assert!(bytes.is_empty(), "body-fact table left trailing bytes");
    for (owner, stored, bytes) in &encoded {
        let decoded = BodyFacts::decode(bytes, &mut resolver)
            .unwrap_or_else(|| panic!("body facts did not decode: {}", prog.def_path(*owner)));
        assert!(stored.same_stored(&decoded), "body facts changed through the store codec: {}", prog.def_path(*owner));
    }
    assert!(!encoded.is_empty(), "no body facts exercised the store codec");
    assert_eq!(failed, 0, "storable test owners had no stable fact encoding");
}

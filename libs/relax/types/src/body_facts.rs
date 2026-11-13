//! What a body check leaves for the crate-wide passes, retained by owner before it is merged.

use crate::borrowck::{event::BorrowError, field_views::{FvRecordRaw, RelaxedRecord}};
use crate::fxhash::{FxBuild, HashMap};
use crate::lint::{AttrKey, Reported};
use crate::lint_dead::BodyInfo;
use crate::program::{capture_import_uses, outside_capture, record_import_uses, DefId, DefKind, ImportUses, Program, Sym};
use relax_session::sync::Mutex;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The body phase's window: item checks before it and crate passes after it keep their sinks.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Facts whose owner scopes have ended, ready for the crate-wide passes after the body phase.
static FACTS: Mutex<HashMap<DefId, BodyFacts>> = Mutex::new(HashMap::with_hasher(FxBuild::new()));
/// Sink writes inside the body phase without an owner: the check tests guard against escapes.
static ESCAPED: AtomicUsize = AtomicUsize::new(0);
/// The checked crates whose items a body should find resolved (fresh: not reused), by crate.
static FRESH: Mutex<Vec<bool>> = Mutex::new(Vec::new());
/// Item cells of a fresh crate first computed inside a body, by `ItemCell`: cells the owners phase
/// did not compute before the bodies ran.
static IN_BODY: [AtomicUsize; ITEM_CELLS] = [const { AtomicUsize::new(0) }; ITEM_CELLS];

thread_local! {
    /// The active owner on this thread, so nested work for the same body shares its buffer.
    static OWNER: Cell<Option<DefId>> = const { Cell::new(None) };
    /// This owner's side effects, kept off the shared sinks until its scope ends.
    static BUFFER: RefCell<BodyFacts> = RefCell::new(BodyFacts::default());
}

/// Names a stored body's definitions, modules, crates and names by index into the store's table
/// (compiler/src/check_bodies.rs); None: the value has no stable name (the body is not stored).
pub trait FactCodec {
    /// Names a definition so its process-local DefId need not be stored.
    fn def(&mut self, d: DefId) -> Option<u32>;
    /// Names a module so another load can find the same scope.
    fn module(&mut self, m: u32) -> Option<u32>;
    /// Names a crate independently of this load's crate order.
    fn krate(&mut self, k: u32) -> Option<u32>;
    /// Names a symbol by text rather than its interner's index.
    fn sym(&mut self, s: Sym) -> Option<u32>;
}

/// The table's entries in the program of the check reading them; None: not in this program.
pub trait FactDecodec {
    /// Resolves a stored definition index for the current program's lint passes.
    fn def(&mut self, i: u32) -> Option<DefId>;
    /// Resolves a stored module index for the current program's import uses.
    fn module(&mut self, i: u32) -> Option<u32>;
    /// Resolves a stored crate index independently of load order.
    fn krate(&mut self, i: u32) -> Option<u32>;
    /// Resolves a stored name without adding symbols to the current program.
    fn sym(&mut self, i: u32) -> Option<Sym>;
}

/// Consumes a tag byte, rejecting a truncated fact before any indexing can panic.
pub(crate) fn read_u8(bytes: &mut &[u8]) -> Option<u8> {
    let (&x, rest) = bytes.split_first()?;
    *bytes = rest;
    Some(x)
}

/// Consumes a little-endian fact word, shared with the private dead-code reference codec.
pub(crate) fn read_u32(bytes: &mut &[u8]) -> Option<u32> {
    let x = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    *bytes = &bytes[4..];
    Some(x)
}

/// One owner's facts in recording order, including its closures and anonymous constants.
#[derive(Default)]
pub struct BodyFacts {
    /// Lint reports with their original sequence numbers, for the check's diagnostic ordering.
    pub reported: Vec<Reported>,
    /// Expectations met by this body, for the later unfulfilled-expectations pass.
    pub fulfilled: Vec<AttrKey>,
    /// References and live-body notes, with the holder used by the dead-code pass.
    pub(crate) refs: Vec<(DefId, BodyInfo)>,
    /// Traits whose methods were used, attributed to the crate whose imports they fulfil.
    pub method_traits: Vec<(u32, DefId)>,
    /// Resolutions through imports, retained without making the front end depend on types.
    pub import_uses: ImportUses,
    /// Conflicts accepted only in relaxed Rust, for the end-of-check report.
    pub fv_only: Vec<BorrowError>,
    /// Field-view rewrites, for the optional strict-source export.
    pub fv_records: Vec<FvRecordRaw>,
    /// Other relaxed acceptances, for strict reports and fixes.
    pub relaxed_records: Vec<RelaxedRecord>,
}

/// Combining scopes preserves each owner's recording order, including work from retries.
impl BodyFacts {
    /// Keeps the position-free facts that a later check can publish, omitting foreign dead refs.
    pub fn stored(&self, prog: &Program, krate: u32) -> Option<BodyFacts> {
        if !self.reported.is_empty() || !self.fulfilled.is_empty() || !self.fv_only.is_empty()
            || !self.fv_records.is_empty() || !self.relaxed_records.is_empty() { return None; }
        Some(BodyFacts {
            refs: self.refs.iter().map(|(holder, info)| Some((*holder, info.stored(prog, krate)?))).collect::<Option<_>>()?,
            method_traits: self.method_traits.clone(),
            import_uses: self.import_uses.clone(),
            ..Default::default()
        })
    }

    /// Keeps the import uses made in modules of the crates `keep` names and the extern names
    /// those crates used: a stored verdict keeps the checked crates' (check_bodies.rs), the only
    /// ones the unused-import lints read (lint_imports.rs). A use in another crate's module (std's)
    /// is made by whichever body first needs that crate's item resolved: no fact of one body.
    pub fn retain_uses(&mut self, prog: &Program, keep: impl Fn(u32) -> bool) {
        let in_kept = |m: u32| keep(prog.mods[m as usize].krate);
        self.import_uses.import.retain(|x| in_kept(x.0));
        self.import_uses.glob.retain(|x| in_kept(x.0));
        self.import_uses.ext.retain(|x| keep(x.0));
    }

    /// Writes facts returned by `stored`; a missing stable name makes the appended bytes unusable.
    pub fn encode(&self, c: &mut dyn FactCodec, out: &mut Vec<u8>) -> bool {
        (|| {
            out.extend_from_slice(&u32::try_from(self.refs.len()).ok()?.to_le_bytes());
            for (holder, info) in &self.refs {
                out.extend_from_slice(&c.def(*holder)?.to_le_bytes());
                info.encode(c, out)?;
            }
            out.extend_from_slice(&u32::try_from(self.method_traits.len()).ok()?.to_le_bytes());
            for &(krate, def) in &self.method_traits {
                out.extend_from_slice(&c.krate(krate)?.to_le_bytes());
                out.extend_from_slice(&c.def(def)?.to_le_bytes());
            }
            out.extend_from_slice(&u32::try_from(self.import_uses.import.len()).ok()?.to_le_bytes());
            for &(module, sym, ns) in &self.import_uses.import {
                out.extend_from_slice(&c.module(module)?.to_le_bytes());
                out.extend_from_slice(&c.sym(sym)?.to_le_bytes());
                out.push(ns as u8);
            }
            out.extend_from_slice(&u32::try_from(self.import_uses.glob.len()).ok()?.to_le_bytes());
            for &(module, def) in &self.import_uses.glob {
                out.extend_from_slice(&c.module(module)?.to_le_bytes());
                out.extend_from_slice(&c.def(def)?.to_le_bytes());
            }
            out.extend_from_slice(&u32::try_from(self.import_uses.ext.len()).ok()?.to_le_bytes());
            for &(krate, sym) in &self.import_uses.ext {
                out.extend_from_slice(&c.krate(krate)?.to_le_bytes());
                out.extend_from_slice(&c.sym(sym)?.to_le_bytes());
            }
            Some(())
        })().is_some()
    }

    /// Resolves a complete stored verdict, rejecting unknown entries, malformed tags and trailing data.
    pub fn decode(mut bytes: &[u8], d: &mut dyn FactDecodec) -> Option<BodyFacts> {
        let mut facts = BodyFacts::default();
        for _ in 0..read_u32(&mut bytes)? {
            let holder = d.def(read_u32(&mut bytes)?)?;
            facts.refs.push((holder, BodyInfo::decode(&mut bytes, d)?));
        }
        for _ in 0..read_u32(&mut bytes)? {
            facts.method_traits.push((d.krate(read_u32(&mut bytes)?)?, d.def(read_u32(&mut bytes)?)?));
        }
        for _ in 0..read_u32(&mut bytes)? {
            let module = d.module(read_u32(&mut bytes)?)?;
            let sym = d.sym(read_u32(&mut bytes)?)?;
            let ns = match read_u8(&mut bytes)? { 0 => false, 1 => true, _ => return None };
            facts.import_uses.import.push((module, sym, ns));
        }
        for _ in 0..read_u32(&mut bytes)? {
            facts.import_uses.glob.push((d.module(read_u32(&mut bytes)?)?, d.def(read_u32(&mut bytes)?)?));
        }
        for _ in 0..read_u32(&mut bytes)? {
            facts.import_uses.ext.push((d.krate(read_u32(&mut bytes)?)?, d.sym(read_u32(&mut bytes)?)?));
        }
        bytes.is_empty().then_some(facts)
    }

    /// The stored parts as sets in a fixed order: refs merged by holder, every list sorted and each
    /// fact once. What the crate passes read of them is set-like (the dead-code worklist's live
    /// set, the used traits and imports), and a check records them in the order its threads met
    /// them, so a stored verdict is written in this form and compared in it.
    pub fn canonical(&self) -> BodyFacts {
        let mut method_traits = self.method_traits.clone();
        method_traits.sort_unstable();
        method_traits.dedup();
        let mut import_uses = self.import_uses.clone();
        import_uses.sort_dedup();
        BodyFacts { refs: crate::lint_dead::canonical_refs(&self.refs), method_traits, import_uses, ..Default::default() }
    }

    /// The first fact only one of two canonical verdicts holds (`self` the stored one, `fresh` the
    /// body's check), described; None when they hold the same.
    pub fn first_difference(&self, prog: &Program, fresh: &BodyFacts) -> Option<String> {
        let side = |stored: bool| if stored { "only in the stored verdict" } else { "only in the body's check" };
        let module = |m: u32| prog.mods.get(m as usize).map_or(format!("#{m}"), |md| format!("`{}`", prog.def_path(md.def)));
        let krate = |k: u32| prog.crates.get(k as usize).map_or(format!("#{k}"), |c| format!("`{}`", prog.syms.str(c.name)));
        if let Some(((holder, r), stored)) = first_only(&crate::lint_dead::ref_pairs(&self.refs), &crate::lint_dead::ref_pairs(&fresh.refs)) {
            return Some(format!("a reference from `{}` to {}: {}", prog.def_path(holder), crate::lint_dead::describe_ref(prog, r), side(stored)));
        }
        if let Some(((k, d), stored)) = first_only(&self.method_traits, &fresh.method_traits) {
            return Some(format!("the method trait `{}` for crate {}: {}", prog.def_path(d), krate(k), side(stored)));
        }
        if let Some(((m, s, ns), stored)) = first_only(&self.import_uses.import, &fresh.import_uses.import) {
            return Some(format!("a use of the import `{}` ({}) in module {}: {}", prog.syms.str(s), if ns { "types" } else { "values" }, module(m), side(stored)));
        }
        if let Some(((m, d), stored)) = first_only(&self.import_uses.glob, &fresh.import_uses.glob) {
            return Some(format!("a use of module {}'s glob import of `{}`: {}", module(m), prog.def_path(d), side(stored)));
        }
        if let Some(((k, s), stored)) = first_only(&self.import_uses.ext, &fresh.import_uses.ext) {
            return Some(format!("a use of the extern name `{}` by crate {}: {}", prog.syms.str(s), krate(k), side(stored)));
        }
        None
    }

    /// Compares the stored parts in recording order without requiring equality on diagnostic state.
    pub fn same_stored(&self, other: &BodyFacts) -> bool {
        self.refs.len() == other.refs.len()
            && self.refs.iter().zip(&other.refs).all(|((a, x), (b, y))| a == b && x.same_stored(y))
            && self.method_traits == other.method_traits && self.import_uses == other.import_uses
    }

    /// Adds a completed scope without reordering the facts already held for this owner.
    fn append(&mut self, mut other: BodyFacts) {
        self.reported.append(&mut other.reported);
        self.fulfilled.append(&mut other.fulfilled);
        self.refs.append(&mut other.refs);
        self.method_traits.append(&mut other.method_traits);
        self.import_uses.append(other.import_uses);
        self.fv_only.append(&mut other.fv_only);
        self.fv_records.append(&mut other.fv_records);
        self.relaxed_records.append(&mut other.relaxed_records);
    }
}

/// The smallest element only one of two sorted, deduplicated lists holds; true: in `a`.
fn first_only<T: Ord + Copy>(a: &[T], b: &[T]) -> Option<(T, bool)> {
    let (mut i, mut j) = (0, 0);
    loop {
        match (a.get(i), b.get(j)) {
            (Some(x), Some(y)) if x == y => { i += 1; j += 1; }
            (Some(x), Some(y)) => return Some(if x < y { (*x, true) } else { (*y, false) }),
            (Some(x), None) => return Some((*x, true)),
            (None, Some(y)) => return Some((*y, false)),
            (None, None) => return None,
        }
    }
}

/// Adds a stored verdict's facts for owner `d` as its check would have left them: check.rs
/// replays a body whose stored verdict stands instead of checking it.
pub fn replay(prog: &Program, d: DefId, facts: BodyFacts) {
    let _k = crate::zone_alloc::kept();
    FACTS.lock().unwrap_or_else(|e| e.into_inner()).entry(fact_owner(prog, d)).or_default().append(facts);
}

/// The lazily computed item cells whose text `item_text` keeps out of body facts.
#[derive(Clone, Copy)]
pub enum ItemCell { Signature, Declaration, ConstDecl, BoundVars, Lifetimes, AliasTy }
const ITEM_CELLS: usize = 6;
const ITEM_CELL_NAMES: [&str; ITEM_CELLS] = ["signatures", "declarations", "const declarations", "bound variables", "lifetime names", "alias types"];

/// Computes item `d`'s cell `f` (its miss path): item text belongs to the text that wrote it, not
/// to the body whose lookup first needed it. Interface text records its import uses outside any
/// body's capture, in the program's sets, as the owners phase does; an item written inside a body
/// (a local struct, fn, impl) is that body's text, so its uses go to that body's facts, whichever
/// body asks. A body's own declarations stay in its capture.
pub fn item_text<R>(prog: &Program, d: DefId, cell: ItemCell, f: impl FnOnce() -> R) -> R {
    if !record_import_uses() { return f(); }
    let key = fact_owner(prog, d);
    let current = OWNER.with(|o| o.get());
    if current == Some(key) { count_in_body(prog, d, cell); return f(); }
    if let Some(root) = hole_owner(prog, key) {
        if current == Some(root) { return f(); }
        let (r, mut uses) = capture_import_uses(f);
        let _k = crate::zone_alloc::kept();
        uses.sort_dedup();
        FACTS.lock().unwrap_or_else(|e| e.into_inner()).entry(root).or_default().import_uses.append(uses);
        return r;
    }
    if current.is_some() { count_in_body(prog, d, cell); }
    outside_capture(f)
}

/// Counts a fresh crate's item cell computed inside a body.
fn count_in_body(prog: &Program, d: DefId, cell: ItemCell) {
    if FRESH.lock().unwrap_or_else(|e| e.into_inner()).get(prog.def(d).krate as usize).copied().unwrap_or(false) {
        IN_BODY[cell as usize].fetch_add(1, Ordering::Relaxed);
    }
}

/// The body owner whose body holds item `d`'s text (the outermost: a fn inside a fn is the outer
/// one's text); None for interface text.
fn hole_owner(prog: &Program, d: DefId) -> Option<DefId> {
    let mut root = None;
    let mut m = prog.def(d).scope;
    while let Some(&f) = prog.block_owner.get(&m) {
        root = Some(f);
        m = prog.def(f).scope;
    }
    root
}

/// Starts a check's body phases: the crates whose items count when a body computes them.
pub fn begin(fresh: &[bool]) {
    *FRESH.lock().unwrap_or_else(|e| e.into_inner()) = fresh.to_vec();
    for n in &IN_BODY { n.store(0, Ordering::Relaxed); }
}

/// The item cells of fresh crates first computed inside a body, in all, then "N kind" per kind.
pub fn items_in_bodies() -> (usize, String) {
    let counts: Vec<usize> = IN_BODY.iter().map(|n| n.load(Ordering::Relaxed)).collect();
    let by_kind: Vec<String> = counts.iter().zip(ITEM_CELL_NAMES).filter(|(n, _)| **n > 0).map(|(n, k)| format!("{n} {k}")).collect();
    (counts.iter().sum(), by_kind.join(", "))
}

/// Starts the body phase; the escape count belongs to this window alone.
pub fn open() { ESCAPED.store(0, Ordering::Relaxed); ACTIVE.store(true, Ordering::Release); }

/// Ends body capture before the crate-wide passes read the merged sinks.
pub fn close() { ACTIVE.store(false, Ordering::Release); }

/// The number of unowned sink writes in this body phase, for the check's test-only guard.
pub fn escaped() -> usize { ESCAPED.load(Ordering::Relaxed) }

/// Child bodies belong to their item; `root_owner` starts at the parent, so items keep themselves.
fn fact_owner(prog: &Program, d: DefId) -> DefId {
    if matches!(prog.def(d).kind, DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy) { crate::borrowck::body::root_owner(prog, d) } else { d }
}

/// Restores a nested owner and publishes this scope's facts even when a body deliberately unwinds.
struct Restore {
    /// The owner whose completed buffer is appended to the store.
    key: DefId,
    /// The enclosing scope's owner, or none when this thread entered its first body.
    owner: Option<DefId>,
    /// The enclosing scope's facts, suspended while a different owner is checked.
    buffer: BodyFacts,
}

/// Scope exit restores the thread before publishing, on both normal return and unwind.
impl Drop for Restore {
    /// Keeps partial facts from a failed attempt just as the former global writes did.
    fn drop(&mut self) {
        let got = BUFFER.with(|b| b.replace(std::mem::take(&mut self.buffer)));
        OWNER.with(|o| o.set(self.owner));
        let _k = crate::zone_alloc::kept();
        FACTS.lock().unwrap_or_else(|e| e.into_inner()).entry(self.key).or_default().append(got);
    }
}

/// Runs a body under its fact owner while the window is open; same-owner queries share its scope.
pub fn with_owner<R>(prog: &Program, d: DefId, f: impl FnOnce() -> R) -> R {
    if !ACTIVE.load(Ordering::Acquire) { return f(); }
    let key = fact_owner(prog, d);
    if OWNER.with(|o| o.get() == Some(key)) { return f(); }
    let _restore = Restore { key, owner: OWNER.with(|o| o.replace(Some(key))), buffer: BUFFER.with(|b| std::mem::take(&mut *b.borrow_mut())) };
    // Catch inside the front-end capture so it returns import uses even on unwind; then pass
    // the same panic on to def_work after those uses join this owner's other partial facts.
    let (r, mut uses) = capture_import_uses(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)));
    {
        let _k = crate::zone_alloc::kept();
        // each use once: the body's memo hits repeat them, and every reader takes a set
        uses.sort_dedup();
        BUFFER.with(|b| b.borrow_mut().import_uses.append(uses));
    }
    match r { Ok(r) => r, Err(p) => std::panic::resume_unwind(p) }
}

/// Records on the current owner, or counts a body-phase escape and leaves the caller its old sink.
pub fn record(f: impl FnOnce(&mut BodyFacts)) -> bool {
    if OWNER.with(|o| o.get().is_some()) {
        let _k = crate::zone_alloc::kept();
        BUFFER.with(|b| f(&mut b.borrow_mut()));
        true
    } else {
        if ACTIVE.load(Ordering::Acquire) { ESCAPED.fetch_add(1, Ordering::Relaxed); }
        false
    }
}

/// Empties the completed body phase in owner order so its facts can be inspected before publication.
pub fn take() -> Vec<(DefId, BodyFacts)> {
    let got = std::mem::take(&mut *FACTS.lock().unwrap_or_else(|e| e.into_inner()));
    let mut owners: Vec<_> = got.into_iter().collect();
    owners.sort_unstable_by_key(|(d, _)| *d);
    owners
}

/// Publishes completed facts before any crate-wide sink reader runs, preserving the supplied order.
pub fn publish(prog: &Program, facts: Vec<(DefId, BodyFacts)>) {
    for (_, f) in facts {
        crate::lint::merge_reported(f.reported);
        crate::lint::merge_fulfilled(f.fulfilled);
        for (holder, info) in f.refs { crate::lint_dead::merge_refs(holder, info); }
        crate::lint_imports::merge_method_traits(f.method_traits);
        prog.add_import_uses(&f.import_uses);
        crate::borrowck::merge_verdict_notes(f.fv_only, f.fv_records, f.relaxed_records);
    }
}

/// What this module holds of a run, as a fresh process has it; no run thread may still use it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&ACTIVE, AtomicBool::new(false));
    reset(&FACTS, Mutex::new(HashMap::default()));
    reset(&ESCAPED, AtomicUsize::new(0));
    reset(&FRESH, Mutex::new(Vec::new()));
    for n in &IN_BODY { n.store(0, Ordering::Relaxed); }
}

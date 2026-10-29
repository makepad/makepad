//! The disk cache of compiled code (plans/relax-cache.md, step C3).
//!
//! A function's machine code is stored with its relocations described by stable identities
//! (a definition by its stable key, a type by its structure, a string literal by its bytes, a
//! panic site by file + position + message, ...), never by this run's numbers. Linking it in a
//! later run resolves every identity to that run's value and writes the address pool. Keys are
//! content hashes: the Relax build, the codegen configuration, the function instance, the
//! interface fingerprints of every crate the instance can depend on (its definition's crate and
//! the crates of its type arguments, with their dependencies; cache/items.rs), and its own
//! body and position. The interface fingerprints carry the bound crates that crate can reach and
//! the bound crates depending on those (items::apply_bound): code that cannot reach a bound crate
//! (std ...) is shared by programs binding other sets; whatever can reach one is shared only by
//! programs with the same view.
//! Never timestamps. An entry also names the slots whose RIR its code took
//! in (inlined bodies, nounwind answers, transitively) with their keys: it links only while
//! those keys are unchanged. So an edit of one fn body recompiles that fn and the code that
//! inlined it, not its crate and every crate above it. Code entries live in per-crate code
//! stores named by the interface fingerprint; front images in the run image stores
//! (cache/front_run.rs), which the stores named by the crate fingerprint refer to (front.rs).
//!
//! MAKEPAD_RUN_CACHE=on (default) | off | verify | readonly;
//! The stores live in the workspace's `<target>/relax/cache` (dirs.rs; MAKEPAD_RUN_CACHE_DIR replaces
//! `<target>/relax` for tests and measurement); MAKEPAD_RUN_CACHE_MAX (MB) caps them (default: twice
//! what the run uses, at least 4096; prune_root).

use crate::borrowck::lazy::StoredVerdict;
use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use crate::jit::{FnKey, Unit};
use crate::pack::{self, NewEntry};
use crate::store::Store;
use crate::program::{DefId, DefKind, NO_DEF};
use relax_backend::rir::{Reloc, RelocAt, RelocKind, Sym};
use crate::sip::Sip128;
use crate::syms::{DataKind, SymKind};
use crate::types::{TyId, TyKind};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

mod collected;
pub use collected::{CollectedCache, CollectedDest, CollectedRecord, CollectedWritten};
mod image;
use image::{ManifestImage, IMAGE_MAGIC};
pub(crate) use image::{complete_identities_unit, ids_report_unit, image_broken_unit, image_slot_by_key_unit, name_unnamed_unit, prelink_unit, PrelinkedSlot};
mod items;
mod deps;
mod demand;
mod front_run;
pub use front_run::FrontSink;
#[cfg(test)]
use front_run::commit_runs;

const KIND_CODE: u32 = 1;
const KIND_DATA: u32 = 2;
const KIND_LEDGER: u32 = 3;
/// part of the codegen configuration hash (store format changes invalidate entries)
const STORE_FORMAT: u32 = 5;
/// borrowck lane: a clean borrow-check verdict (empty payload)
const KIND_BORROWCK: u32 = 4;
/// the checker's own version: bump when its rules change (old verdicts are then ignored)
pub const BORROWCK_VERSION: u32 = 2;
/// the coherence check's own version: bump when its rules change
const COHERENCE_VERSION: u32 = 2;

/// Immutable driver view for Gandalf's generic owner discovery. No Unit/cache
/// callbacks under the front read lock, and no need to construct an instance key
/// (which could itself demand closure capture analysis): owner verdicts are read from the
/// store they are written to (record_borrowck: an owner key names no crate, so crate 0's
/// code store).
pub struct OwnerVerdicts {
    cfg: u128,
    fps: Vec<u128>,
    /// interface fingerprints: they carry which crates the loader binds (items::crate_ifaces)
    ifaces: Vec<u128>,
    epoch: u64,
    store: Option<std::sync::Arc<Store>>,
}

impl OwnerVerdicts {
    fn new(cfg: u128, fps: Vec<u128>, ifaces: Vec<u128>, epoch: u64, store: Option<std::sync::Arc<Store>>) -> Self {
        Self { cfg, fps, ifaces, epoch, store }
    }
    pub fn lookup(&self, prog: &crate::program::Program, def: DefId, epoch: u64) -> (Option<u128>, Option<StoredVerdict>) {
        // Startup fingerprints cannot validate live edits or subsequently loaded modules: a
        // crate that a live edit changed, or that depends on one, is checked again; the others'
        // fingerprints still hold (a runner session edits one app, not every crate).
        if epoch != self.epoch { return (None, None); }
        let krate = prog.def(def).krate;
        let edited = crate::reload::edited_crates();
        if !edited.is_empty() && reaches_edited(prog, krate, &edited) { return (None, None); }
        let (Some(fp), Some(iface)) = (self.fps.get(krate as usize), self.ifaces.get(krate as usize)) else { return (None, None) };
        let mut h = Sip128::new();
        h.str("borrowck-owner");
        h.u32(BORROWCK_VERSION);
        h.u128(self.cfg);
        h.u128(*fp); // includes the crate's transitive dependencies
        h.u128(*iface); // ... and which of them the loader binds
        crate::stable::def_ident(prog, def, &mut h);
        let key = h.finish128();
        let stored = match self.store.as_ref().and_then(|st| st.get(key)) {
            Some((KIND_BORROWCK, p)) => dec_verdict(p),
            _ => None,
        };
        (Some(key), stored)
    }
}

/// Whether crate `krate` is one of `edited` or depends on one, transitively.
fn reaches_edited(prog: &crate::program::Program, krate: u32, edited: &[u32]) -> bool {
    let mut seen = vec![false; prog.crates.len()];
    let mut todo = vec![krate];
    while let Some(c) = todo.pop() {
        if edited.contains(&c) { return true; }
        let Some(ci) = prog.crates.get(c as usize) else { return true };
        if std::mem::replace(&mut seen[c as usize], true) { continue; }
        todo.extend(ci.deps.iter().map(|(_, d)| *d));
    }
    false
}

/// A stored borrow-check verdict's payload: empty = Clean, 1 + reason = Unchecked.
fn enc_verdict(v: &StoredVerdict) -> Vec<u8> {
    match v {
        StoredVerdict::Clean => Vec::new(),
        StoredVerdict::Unchecked(why) => std::iter::once(1u8).chain(why.bytes()).collect(),
    }
}
fn dec_verdict(p: &[u8]) -> Option<StoredVerdict> {
    match p.split_first() {
        None => Some(StoredVerdict::Clean),
        Some((1, why)) => Some(StoredVerdict::Unchecked(String::from_utf8(why.to_vec()).ok()?)),
        Some(_) => None,
    }
}
/// the slots a program's start installed, in order, with the code entries they linked (startup
/// slot manifest; per program + features)
const KIND_MANIFEST: u32 = 5;
/// a front-end image (front.rs): a file's in a run store (front_run.rs), a demand part's
/// surface or definition in a crate store
const KIND_FRONT: u32 = 6;
/// a crate store's reference to a file's image in a run store (front_run.rs), keyed by the
/// file's path and source (Cache::front_key / front_part_key)
const KIND_FRONT_REF: u32 = 14;
const KIND_FRONT_CRATE: u32 = 7;
const KIND_FRONT_GRAPH: u32 = 8;
const KIND_DEMAND_PART: u32 = 13;
/// a graph store's list of the crate stores its demand parts take images from (prune keeps them)
const KIND_DEMAND_STORES: u32 = 16;
/// the names a fn body's macro expansions produced (cache/deps.rs exp_names), keyed by the code
/// key the fn has without them: a later run keys the fn's code without parsing its body
const KIND_EXP_NAMES: u32 = 17;
/// the program's item graph (cache/deps.rs Deps::encode), one per program (its root crate's code
/// store, under a key of the program's view: replaced when the program changes)
const KIND_DEPS: u32 = 18;
/// a program's last load order (cache.rs load_order): its crates (name, root), in order, keyed by
/// the graph's crate set (GraphRecipe::set)
const KIND_FRONT_ORDER: u32 = 19;
const KIND_CONST: u32 = 9;
const KIND_WARM_OUTCOME: u32 = 10;
/// stores a front image was mapped from: kept for the rest of the process (the arrays point
/// into their mappings)
static FRONT_STORES: relax_session::sync::Mutex<Vec<std::sync::Arc<Store>>> = relax_session::sync::Mutex::new(Vec::new());
/// front images this run mapped (store, index entry): checksummed in the background
static FRONT_USED: relax_session::sync::Mutex<Vec<(std::sync::Arc<Store>, usize)>> = relax_session::sync::Mutex::new(Vec::new());

/// Pins a store a front image or collected image was mapped from (FRONT_STORES), and with `used`
/// queues that entry's background checksum (FRONT_USED).
fn pin_front(st: &std::sync::Arc<Store>, used: Option<usize>) {
    let mut keep = FRONT_STORES.lock().unwrap_or_else(|x| x.into_inner());
    if !keep.iter().any(|k| std::sync::Arc::ptr_eq(k, st)) {
        keep.push(st.clone());
    }
    drop(keep);
    if let Some(i) = used {
        FRONT_USED.lock().unwrap_or_else(|x| x.into_inner()).push((st.clone(), i));
    }
}

/// Checksums the front images this run mapped, on a low-priority background thread (after the
/// start). A mismatch -- a store damaged on disk -- removes that store file, so the next run
/// parses and compiles that crate afresh; this run already validated the images' structure
/// (offsets, lengths, alignment) and, against its own parse, their source bytes.
pub fn verify_front_images() {
    let used = std::mem::take(&mut *FRONT_USED.lock().unwrap_or_else(|x| x.into_inner()));
    if used.is_empty() {
        return;
    }
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-cache-verify".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
        crate::os::thread::lower_priority(true);
        let mut bad: Vec<std::path::PathBuf> = Vec::new();
        for (st, i) in &used {
            if !st.verify_at(*i) && !bad.contains(&st.path) {
                bad.push(st.path.clone());
            }
        }
        for p in bad {
            eprintln!("makepad-run cache: a front image in {} does not match its checksum: the store is removed (the next run rebuilds it)", p.display());
            let _ = std::fs::remove_file(crate::store::current(&p));
        }
    });
    }
}
const LEDGER_KEY: u128 = 0;
// N01 types and closure arguments with baseline tiers and closure fingerprints; 26: a writable block
// symbol names which block of its entry it is (its ordinal), so two relocations to different
// offsets of one block link to one block. 29: image symbol and closure-parent key spans (image.rs
// skip_*) follow the encoding again; images written before cut keys short.
const ENTRY_VERSION: u32 = 29;
/// the level of a code entry's code (its tier byte)
const TIER_O1: u8 = 0;
const TIER_O3: u8 = 1;
const TIER_O2: u8 = 2;
/// baseline code (a build before the first present: unoptimised RIR, no inlining of user fns):
/// a run that links it builds it again at O2 after its first present (jit.rs note_base), and
/// that code replaces the entry
const TIER_BASE: u8 = 3;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    On,
    Verify,
    ReadOnly,
}

/// MAKEPAD_RUN_CACHE: unset / `on` = on (the default), `off` / `0`, `verify`, `readonly`.
pub fn mode_from_env(options: &crate::options::Options) -> Option<Mode> {
    // A process-wide optimization budget depends on which functions ran before this one.
    // rustc: rustc_mir_transform/src/pass_manager.rs:298 (MIR optimization bisection).
    if std::env::var_os("MAKEPAD_RUN_THREAD_MAX").is_some() {
        if options.verbose {
            eprintln!("makepad-run cache: disabled because MAKEPAD_RUN_THREAD_MAX uses a process-wide optimization budget");
        }
        return None;
    }
    // coverage stores name this process's table (coverage.rs): such code is never cached
    if crate::coverage::on() {
        return None;
    }
    match std::env::var("MAKEPAD_RUN_CACHE").ok().as_deref() {
        Some("off") | Some("0") => None,
        Some("verify") => Some(Mode::Verify),
        Some("readonly") => Some(Mode::ReadOnly),
        _ => Some(Mode::On),
    }
}

/// This process's warm-cache outcomes, readable without the unit (`makepad inspect cache`, flight
/// recordings' `jit.cache` event): code lookups by outcome, the startup image, the stores opened.
pub mod warm_count {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    /// a lookup linked the cached entry
    pub const HIT: usize = 0;
    /// the fn has no storable key (nothing can be cached for it)
    pub const NO_KEY: usize = 1;
    /// no entry under the fn's key: never recorded, recorded under another key (a source or
    /// dependency changed), or not written (the session that compiled it did not flush)
    pub const NO_ENTRY: usize = 2;
    /// an entry, invalidated: a slot its code took in has another key now (certificate)
    pub const INVALIDATED: usize = 3;
    /// an entry that did not decode or link
    pub const LINK_FAIL: usize = 4;
    /// startup image: slots placed, stale (their crate changed or the cache has newer code),
    /// failed (an identity did not resolve / link), had code already
    pub const IMG_PLACED: usize = 5;
    pub const IMG_STALE: usize = 6;
    pub const IMG_FAILED: usize = 7;
    pub const IMG_HAD_CODE: usize = 8;
    /// code stores opened, crate stores rejected (changed include!/env! inputs, a re-parsed crate)
    pub const STORES: usize = 9;
    pub const STORES_REJECTED: usize = 10;
    /// entries recorded this session (written by the batched record writes)
    pub const RECORDED: usize = 11;
    /// strict runs (MAKEPAD_RUN_WARM_STRICT=1) split NO_ENTRY by the cold run's ledger: recorded
    /// under another key, recorded but its entry unavailable, never recorded (with a reason)
    pub const KEY_CHANGED: usize = 12;
    pub const ENTRY_GONE: usize = 13;
    pub const NOT_RECORDED: usize = 14;
    /// this session's installed O3 code and its entry: queued for the record batches, or not
    /// stored because it was unverified (SPEC-V), its frame too big, speculative / an inspect build
    pub const O3_QUEUED: usize = 15;
    pub const O3_UNVERIFIED: usize = 16;
    pub const O3_FRAME: usize = 17;
    pub const O3_SPEC: usize = 18;
    /// ... and of the queued: written as the slot's entry, the same bytes as its entry already,
    /// not encodable, dropped (a source changed or the slot was patched since the install)
    pub const O3_WRITTEN: usize = 19;
    pub const O3_SAME: usize = 20;
    pub const O3_UNENCODABLE: usize = 21;
    pub const O3_STALE: usize = 22;
    /// O2 code written as the slot's entry
    pub const O2_WRITTEN: usize = 23;
    /// a warm session: fresh O3 installs by the tier of the slot's
    /// entry in the stores this session opened (none, baseline, O1, O2, O3): what a tier-up rebuilt
    /// over (an O3 entry means the earlier session's O3 code was there but not what the slot ran)
    pub const RETIER_O3_OVER: usize = 24;
    /// ... and fresh O2 builds (the O2 worker's) the same way
    pub const RETIER_O2_OVER: usize = 29;
    /// queued O3 records not stored: the slot was patched / an edit version / it has no key
    pub const O3_DROPPED: usize = 34;
    /// deferred records waiting for the record batches now (set by the batches and the installs)
    pub const RECORDS_WAITING: usize = 35;
    /// (diagnostic) why the last record drain stopped (1 stopped, 2 no unit, 3 cache out, 4 other unit, 5 ran) and its unit's backlog
    pub const DRAIN_WHY: usize = 36;
    pub const DRAIN_LEN: usize = 37;
    /// SPEC-V: code records made from an unverified lowering (baseline, calls to twins), and
    /// compiled slots below O3 whose verdict came clean after their code was built (their code
    /// and entry stay the unverified lowering)
    pub const REC_UNVERIFIED: usize = 38;
    pub const SPECV_LATE: usize = 39;
    /// verified O1 rebuilds (jit.rs o1v): queued because the verdict came late / because O3
    /// inlined a slot with no standalone code; built; written as the slot's entry; dropped
    /// (not verified, O3 meanwhile, entry already O1+, build failed)
    pub const O1V_LATE: usize = 40;
    pub const O1V_INLINED: usize = 41;
    pub const O1V_BUILT: usize = 42;
    pub const O1V_WRITTEN: usize = 43;
    pub const O1V_DROPPED: usize = 44;
    /// SPEC-V: bodies with no entry of their own that linked their twin's stored entry
    pub const TWIN_ENTRY: usize = 45;
    pub static N: [AtomicU64; 46] = [const { AtomicU64::new(0) }; 46];
    #[inline]
    pub fn add(k: usize, n: u64) {
        N[k].fetch_add(n, Relaxed);
    }
    pub fn get(k: usize) -> u64 {
        N[k].load(Relaxed)
    }
}

/// The warm cache this session in one paragraph (answer first: hits vs misses by reason).
pub fn warm_summary() -> String {
    use warm_count::*;
    let g = |k| get(k);
    let k = |n: u64| if n >= 10_000 { format!("{:.1}k", n as f64 / 1e3) } else { n.to_string() };
    let miss = g(NO_KEY) + g(NO_ENTRY) + g(INVALIDATED) + g(LINK_FAIL);
    let mut s = if g(STORES) == 0 && g(HIT) == 0 {
        "warm cache: no code store opened for this build (the first run of this loader build, the cache off, or another cache home): every fn compiled fresh".to_string()
    } else {
        format!("warm cache: {} lookups linked cached code, {} missed", k(g(HIT)), k(miss))
    };
    if miss > 0 {
        let mut why = vec![format!("no entry for the key {}", k(g(NO_ENTRY)))];
        if g(KEY_CHANGED) + g(ENTRY_GONE) + g(NOT_RECORDED) > 0 {
            why.push(format!("of them (strict ledger) recorded under another key {}, recorded but gone {}, never recorded {}", k(g(KEY_CHANGED)), k(g(ENTRY_GONE)), k(g(NOT_RECORDED))));
        }
        why.push(format!("invalidated (a slot its code took in changed) {}", k(g(INVALIDATED))));
        why.push(format!("did not decode/link {}", k(g(LINK_FAIL))));
        why.push(format!("fn not storable {}", k(g(NO_KEY))));
        s.push_str(&format!(" ({})", why.join(", ")));
    }
    if g(TWIN_ENTRY) > 0 {
        s.push_str(&format!("; {} bodies without an entry of their own ran their twin's stored code", k(g(TWIN_ENTRY))));
    }
    if g(IMG_PLACED) + g(IMG_STALE) + g(IMG_FAILED) > 0 {
        s.push_str(&format!("; startup image: {} slots placed, {} stale, {} failed, {} had code", k(g(IMG_PLACED)), k(g(IMG_STALE)), k(g(IMG_FAILED)), k(g(IMG_HAD_CODE))));
    }
    s.push_str(&format!("; {} code stores opened, {} crate stores rejected; {} entries recorded this session", g(STORES), g(STORES_REJECTED), k(g(RECORDED))));
    let over = |b: usize| (0..5).map(|i| k(g(b + i))).collect::<Vec<_>>().join("/");
    if (0..10).any(|i| g(RETIER_O3_OVER + i) > 0) {
        s.push_str(&format!("; fresh builds over the stored entry (none/baseline/O1/O2/O3): O3 {}, O2 {}", over(RETIER_O3_OVER), over(RETIER_O2_OVER)));
    }
    if g(REC_UNVERIFIED) + g(SPECV_LATE) > 0 {
        s.push_str(&format!("; SPEC-V: {} records of unverified lowerings, {} compiled slots below O3 verified after their build, {} base slots ran their twins' code until their own build (none compiled on a program thread for it), {} of them have their own code now, {} callees built ahead of a replacement's install, {} calls of linked code sent to a twin's base, {} bases of hot twins built for their own entry", k(g(REC_UNVERIFIED)), k(g(SPECV_LATE)), k(crate::jit::SPECV_BORROWS.load(std::sync::atomic::Ordering::Relaxed)), k(crate::jit::SPECV_BORROWS_OWN.load(std::sync::atomic::Ordering::Relaxed)), k(crate::jit::SPECV_PREBUILT.load(std::sync::atomic::Ordering::Relaxed)), k(crate::jit::TWIN_RETARGETS.load(std::sync::atomic::Ordering::Relaxed)), k(crate::jit::TWIN_BASE_BUILDS.load(std::sync::atomic::Ordering::Relaxed))));
    }
    if g(O3_QUEUED) + g(O3_UNVERIFIED) + g(O3_FRAME) + g(O3_SPEC) > 0 {
        s.push_str(&format!("; O3 installs kept as entries: {} queued (written {}, same as stored {}, not encodable {}, stale {}, dropped {}; {} deferred records waiting, last drain {} with {}), not kept: unverified {}, frame too big {}, speculative/inspect {}; O2 entries written {}",
            k(g(O3_QUEUED)), k(g(O3_WRITTEN)), k(g(O3_SAME)), k(g(O3_UNENCODABLE)), k(g(O3_STALE)), k(g(O3_DROPPED)), k(g(RECORDS_WAITING)), g(DRAIN_WHY), k(g(DRAIN_LEN)), k(g(O3_UNVERIFIED)), k(g(O3_FRAME)), k(g(O3_SPEC)), k(g(O2_WRITTEN))));
    }
    if g(O1V_LATE) + g(O1V_INLINED) > 0 {
        s.push_str(&format!("; verified O1 rebuilds: queued {} (verdict late) + {} (inlined-only), built {}, written {}, dropped {}",
            k(g(O1V_LATE)), k(g(O1V_INLINED)), k(g(O1V_BUILT)), k(g(O1V_WRITTEN)), k(g(O1V_DROPPED))));
    }
    if g(NO_ENTRY) > 0 && g(KEY_CHANGED) + g(ENTRY_GONE) + g(NOT_RECORDED) == 0 {
        s.push_str(" (why an entry is missing -- key changed, never recorded, not flushed -- needs a MAKEPAD_RUN_WARM_STRICT=1 cold run then warm run)");
    }
    s
}

#[derive(Default, Debug, Clone)]
pub struct CacheStats {
    /// replacing records: [without a stored entry (the startup image is written again), moving the
    /// slot's tier up (counted like stale image slots)]
    pub rewrite_why: [u64; 2],
    pub hits: u64,
    pub misses: u64,
    pub link_fail: u64,
    pub recorded: u64,
    pub unencodable: u64,
    /// entries rejected because a closure they name is another expression in this parse
    pub closure_mismatch: u64,
    pub link_ns: u64,
    pub verify_same: u64,
    pub verify_diff: u64,
    pub packs: u64,
    pub packs_rejected: u64,
    pub index_entries: u64,
    pub open_ns: u64,
    pub def_collisions: u64,
    /// hits linked by speculative workers (off the program's threads) and their link cpu
    pub spec_hits: u64,
    pub spec_link_ns: u64,
    /// entries not linked because a slot their code took in changed (its key differs)
    pub cert_rejects: u64,
    /// code stores opened (the front stores are `packs`)
    pub code_stores: u64,
    /// ms computing file digests and interface fingerprints at open
    pub iface_ns: u64,
    /// cpu of the per-item work: definition body/position digests (code keys), certificates
    /// written (record) and checked (lookup)
    pub body_ns: u64,
    pub cert_write_ns: u64,
    pub cert_check_ns: u64,
    /// the startup image (`prelink`): slots installed from it, slots left lazy because their code
    /// is stale (a crate it is made from changed, or the cache has tier-1 code for it by now),
    /// because an identity did not resolve (or the code did not link), or because they had code
    /// already; wall ns of the whole prelink
    pub prelinked: u64,
    pub prelink_changed: u64,
    /// image slots naming a changed crate prelinked after all (their key and certificate as
    /// recorded), and the ns their check took
    pub prelink_rekeyed: u64,
    pub prelink_check_ns: u64,
    pub prelink_failed: u64,
    pub prelink_had_code: u64,
    pub prelink_ns: u64,
    /// ... of it: reading the image, making the slots its keys name, resolving its symbols
    pub prelink_parse_ns: u64,
    pub prelink_keys_ns: u64,
    pub prelink_syms_ns: u64,
    /// data objects placed from the image (place_data)
    pub prelink_data: u64,
    /// the image slots' identities decoded after the prelink (`complete_identities`): decoded,
    /// of them a key an earlier slot already had (the slot stays, unmapped), keys that did not
    /// decode, and the wall ns it took (in the background and on the thread that finished it)
    pub ids_decoded: u64,
    pub ids_dup: u64,
    pub ids_failed: u64,
    pub ids_ns: u64,
    pub ids_bg: u64,
    /// of them, decoded in the prelink because their keys name a changed crate, and its wall ns
    pub ids_unsure: u64,
    pub ids_unsure_ns: u64,
}

/// Interior storage for the cache installed in a Unit. The owner thread protects
/// every access. Linker borrows one operation at a time and holds only this cell's
/// raw pointer across callbacks, which may recursively access the same cell.
pub struct CacheCell(std::cell::UnsafeCell<Cache>);
// All cache reads and writes are serialized by the owner thread. Off-unit compiler
// work can share a Unit, but does not access its cache.
unsafe impl Sync for CacheCell {}
impl CacheCell {
    pub fn new(cache: Cache) -> Self { Self(std::cell::UnsafeCell::new(cache)) }
    pub(crate) fn get(&self) -> *mut Cache { self.0.get() }
}
impl std::ops::Deref for CacheCell {
    type Target = Cache;
    fn deref(&self) -> &Cache { unsafe { &*self.get() } }
}
impl std::ops::DerefMut for CacheCell {
    fn deref_mut(&mut self) -> &mut Cache { unsafe { &mut *self.get() } }
}

/// The install-time inputs of a code entry that cannot be recovered later (`Cache::defer_record`).
pub(crate) struct PendingRecord {
    id: u32,
    code: Vec<u8>,
    relocs: Vec<RelocAt>,
    pc_map: Vec<(u32, u64)>,
    nounwind: Vec<(u32, bool)>,
    baseline: bool,
    deps: Vec<u32>,
    /// jit::patch_epoch() at the install: a record is encoded only against the sources it was built from
    epoch: u64,
    /// O3 code: its inlined slots and nounwind inputs (`record_o3`)
    o3: Option<(Vec<u32>, Vec<(u32, bool)>)>,
}

/// Is a deferred record still describing the sources its code was built from?
fn pending_current(u: &Unit, p: &PendingRecord) -> bool {
    let f = &u.fns[p.id as usize];
    p.epoch == crate::jit::patch_epoch() && f.patch_gen == 0 && !u.prog.is_edit_version(f.file)
}

/// The slots whose RIR the code of `id` took in (`Unit::body_deps` from `id` and `extra`,
/// transitively), without `id` itself.
/// What code addressing a `#[thread_local]` static depends on of its place.
fn tls_shape(pl: &crate::jit::TlsPlace) -> u8 {
    pl.fixed as u8 | (pl.init.is_some() as u8) << 1
}

fn record_deps(u: &Unit, id: u32, extra: &[u32]) -> Vec<u32> {
    let mut stack: Vec<u32> = extra.iter().copied().chain(u.body_deps.get(&id).into_iter().flatten().copied()).collect();
    let mut deps = Vec::new();
    if stack.is_empty() { return deps; }
    let mut seen = HashSet::new();
    seen.insert(id);
    while let Some(c) = stack.pop() {
        if !seen.insert(c) || c as usize >= u.fns.len() { continue; }
        if let Some(v) = u.body_deps.get(&c) { stack.extend(v.iter().copied()); }
        deps.push(c);
    }
    deps
}

pub struct Cache {
    pub mode: Mode,
    verbose: bool,
    /// the `<relax dir>/cache` folder: one store file per crate version (store.rs)
    dir: PathBuf,
    /// the opened store files
    stores: Vec<std::sync::Arc<Store>>,
    /// per crate: its store file's path, and whether that file's contents are stale (a late
    /// input changed: the next write replaces it whole)
    crate_path: Vec<PathBuf>,
    stale: Vec<bool>,
    /// per crate: dependency depth (an entry goes to the deepest crate its key names)
    depth: Vec<u32>,
    /// key -> the crate whose store file the entry goes to
    key_crate: HashMap<u128, u32>,
    /// the crate of the entry being recorded (its data objects go to the same file)
    cur_crate: u32,
    /// key -> (store, its index entry)
    index: KeyMap<(u32, u32)>,
    mapped_index: Option<collected::MappedIndexes>,
    /// crate fingerprints (stable.rs), per crate index
    fps: Vec<u128>,
    /// interface fingerprints (cache/items.rs), per crate index: what code keys name; the bound
    /// crates of this program folded in (items::apply_bound)
    iface: Vec<u128>,
    /// the same without the bound crates: what a collected image stores (run, check and
    /// check --boundary of one program bind different sets, the image is shared by all)
    iface_plain: Vec<u128>,
    /// per crate: its view (items.rs crate_views, bound crates folded in): what code keys and
    /// store names take from the crate besides its items
    view: Vec<u128>,
    /// the program's item graph (cache/deps.rs), built on the first key that needs it
    deps: Option<std::sync::Arc<deps::Deps>>,
    /// (crate, name) -> what the name finds from the crate (deps.rs name_closure)
    name_memo: HashMap<(u32, deps::Name), u128>,
    /// per definition: its body's names, as one closure hash (code keys)
    body_names: HashMap<DefId, u128>,
    /// per file: its interface hash and the fn bodies outside it
    digests: Vec<Option<std::sync::Arc<items::FileDigest>>>,
    /// per definition: its body (when a hole) and position digest, hashed (code keys)
    def_bodies: HashMap<DefId, u128>,
    /// per crate: its code store (named by the interface fingerprint), and whether that file's
    /// contents are stale (a late input changed)
    code_path: Vec<PathBuf>,
    code_stale: Vec<bool>,
    /// code keys whose entry this run rejected (a dependency's key changed): recorded again
    cert_rejected: HashSet<u128>,
    /// code keys of bodies the borrow checker rejected this run (reject_verdict): never recorded
    verdict_rejected: HashSet<u128>,
    /// per slot: the crates its key names (a certificate's fast path)
    slot_crates: HashMap<u32, Vec<u32>>,
    /// value entries (record_const) whose stored certificate no longer holds: recorded again
    const_stale: HashSet<u128>,
    /// value entries this run validated or recorded: the startup image takes only these
    const_live: HashSet<u128>,
    /// the code keys of the bodies value certificates name (code_key_bytes by identity)
    const_body_keys: HashMap<FnKey, Option<(u128, Vec<u32>, Vec<u8>)>>,
    /// per crate: the indexes of its opened crate stores (has_front_key_in_crate)
    front_stores_of: Vec<Vec<u32>>,
    front_stores_n: usize,
    /// while a key is encoded: the definitions it names (their local bodies join the key)
    enc_defs: Option<Vec<DefId>>,
    /// crate identity hash -> crate index
    crate_by_ident: HashMap<u128, u32>,
    crate_ident: Vec<u128>,
    /// codegen identity (codegen_id.rs) + codegen configuration
    cfg: u128,
    pending: Vec<(u32, NewEntry)>,
    /// payload bytes in `pending` (spill_take: written out during the run past a threshold)
    pending_bytes: usize,
    pending_keys: HashSet<u128>,
    /// Exit refuses new key queries (slot_key); a deferred record's keys are still wanted while
    /// the exit drains them (record_pending sets this for its own encoding)
    recording_pending: bool,
    /// front images (crate, key) reserved this run: an image is local to its crate's store
    front_reserved: HashSet<(u32, u128)>,
    /// link name -> canonical foreign declaration (canon_foreign; built on first use)
    foreign_canon: Option<HashMap<String, DefId>>,
    /// per crate: definition key -> DefId (built on the first decode in that crate)
    def_maps: HashMap<u32, HashMap<u128, DefId>>,
    /// every definition by crate (built once, on the first decode)
    crate_defs: Vec<Vec<DefId>>,
    crate_defs_n: usize,
    /// per def: its key (memo, hierarchical)
    def_keys: HashMap<DefId, u128>,
    mapped_definitions: Option<collected::MappedDefinitions>,
    /// Frozen before the first run-time crate mutates the boot's front end.
    collected_boot: Option<collected::CollectedBoot>,
    collected_committed: bool,
    file_ids: HashMap<u128, u32>,
    file_ids_n: usize,
    file_hash: HashMap<u32, u128>,
    /// data entry key -> address in this run
    data_addr: HashMap<u128, u64>,
    /// data object key -> its per-domain copy this run (link_symbol tag 10)
    const_copy: HashMap<u128, u64>,
    /// data object index -> its entry key (None: cannot be stored)
    data_key: HashMap<u32, Option<u128>>,
    /// why a data object cannot be stored (memo of data_entry's reason)
    data_why: HashMap<u32, &'static str>,
    /// slots whose code came from the cache this run (not recorded again)
    linked: HashSet<u32>,
    /// slots looked up without a usable entry this run (the index does not change in a run)
    missed: HashSet<u32>,
    linking: HashSet<u32>,
    linking_data: HashSet<u128>,
    /// the slots the last linked entry calls (speculative linking follows them)
    pub last_callees: Vec<u32>,
    /// the slots whose address the last linked entry takes, and the data objects it reads
    /// (real-time code's reach: jit.rs realtime)
    pub last_thunks: Vec<u32>,
    pub last_data: Vec<u64>,
    /// the hit just linked is O3 code; the slots its body inlined (live patches of those
    /// rebuild it)
    pub last_tier1: Option<Vec<u32>>,
    /// the hit just linked is O2 code (a `#[relax(O2)]` slot's, or a slot an earlier run found
    /// hot): no O2 build for it this run
    pub last_o2: bool,
    /// the entry just decoded is baseline code (TIER_BASE): lookup moves it to `base_linked`
    pub last_base: bool,
    /// slots linked from TIER_BASE entries and not installed yet (jit.rs install_compiled)
    pub base_linked: HashSet<u32>,
    /// the previous run's startup manifest (slot key encodings in install order)
    pub manifest: Vec<Vec<u8>>,
    /// how many of `manifest`'s slots were installed before the first present
    pub manifest_present: usize,
    /// the manifest's slots whose recorded code entry is no longer in the index (any: the
    /// manifest does not describe this cache, and the start is not warm)
    pub manifest_missing: usize,
    /// the manifest's slots that had no code entry when it was recorded (they compile)
    pub manifest_uncached: usize,
    /// of the slots before the first present: how many had a code entry when it was recorded
    manifest_present_cached: usize,
    /// the loaded manifest's code keys by the hash of their slot's identity bytes: a slot's code
    /// key is taken as recorded instead of hashing the bodies it is made from again
    /// (code_key_bytes) when every crate its identity names is as the manifest was recorded
    /// from (`manifest_key_crates`). Empty without a manifest whose entries are all here.
    manifest_keys: HashMap<u128, u128>,
    /// per crate: unchanged since the manifest was recorded (its fingerprint, which covers its
    /// dependencies, is the one in the manifest's startup image); None: every crate (the manifest
    /// was recorded from exactly this run's sources, manifest_sources_key). A slot whose crates
    /// are all unchanged has its recorded key -- the startup image's rule for prelinking its code.
    manifest_key_crates: Option<Vec<bool>>,
    /// the loaded manifest's code keys, in `manifest`'s order (0: none): a slot whose key this
    /// run's manifest changes had its old entry superseded (record_manifest removes it)
    manifest_old_keys: Vec<u128>,
    /// the manifest's prelinked startup image (the code of its slots before the first present,
    /// stored once relocatable against a table of this run's identities: `prelink`)
    image: Option<ManifestImage>,
    /// the slots the prelinked startup image's keys name in this run (image.rs)
    image_ids: Option<image::ImageIds>,
    /// the manifest is written again at exit even when this run's start matches it: it has no
    /// image, or one this run could not use
    image_rewrite: bool,
    /// record_manifest ran this run (the mid-run write): the exit writes the manifest again only
    /// if `image_rewrite` was set since (an O2/O3 entry replaced code the image may hold)
    manifest_done: bool,
    /// the slot count of the order this run's manifest was last made from: a longer one (the
    /// session's later installs) is recorded again
    manifest_len: usize,
    /// the loaded image's slots, and how many of them this run found stale (left lazy: a crate
    /// changed, an identity did not resolve) -- past 2% (as for the manifest) the next manifest
    /// carries a fresh image
    image_slots: usize,
    image_stale: usize,
    /// the tier of each code entry this run recorded (by key), pending or written: a verified O1
    /// rebuild replaces only an entry below O1
    rec_tier: HashMap<u128, u8>,
    /// code reached a slot of the loaded image without an identity (`image_broken`): the
    /// manifest carries no image from now on (written at once, at exit too), so no later run
    /// starts from it
    image_bad: bool,
    /// the root crate whose manifest load_manifest read
    manifest_root: Option<u32>,
    /// slot -> the code entry its code was linked from this run (lazily or prelinked): the next
    /// manifest's image takes that entry
    linked_key: HashMap<u32, u128>,
    /// slot keys by slot (fixed for a run)
    slot_keys: HashMap<u32, Option<u128>>,
    /// An unnamed image slot must not cache a failed key before its identity is decoded.
    key_waits: bool,
    /// a slot's encoded key (None: not encodable), and encoded keys already decoded to slots
    fnkey_bytes: HashMap<u32, Option<Vec<u8>>>,
    /// borrow-check verdicts recorded by this run (not flushed yet)
    pending_verdicts: HashMap<u128, StoredVerdict>,
    slot_by_bytes: crate::fxhash::HashMap<Box<[u8]>, u32>,
    /// (file, closure expr) -> the identity (closure_text_fp) a decoded key or entry said the
    /// expression has, for expressions this run has not parsed yet: checked when one is
    /// decoded again and when the closure is built (jit: closure_identity_ok)
    closure_fps: HashMap<(u32, u32), u64>,
    pub stats: CacheStats,
    pub why_unencodable: HashMap<&'static str, u64>,
    last_why: &'static str,
}

type LinkedCode = (Vec<u8>, Vec<(u32, u64)>, Vec<RelocAt>);

// ------------------------------------------------------------ byte encoding

#[derive(Default)]
struct E(Vec<u8>);

impl E {
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u128(&mut self, x: u128) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
}

struct D<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> D<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// the u32 at the current position, without advancing
    fn clone_u32_at_rest(&self) -> u32 {
        self.b.get(self.p..self.p + 4).map_or(0, |x| u32::from_le_bytes(x.try_into().unwrap()))
    }

    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn str(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }
}

fn hash_bytes(b: &[u8]) -> u128 {
    crate::sip::hash_bytes(b)
}

/// What opening a cache reads of a unit: its program and types, and the settings the cache is
/// keyed by (OpenFrom::of)
pub struct OpenFrom<'a> {
    pub prog: &'a crate::program::Program,
    pub tcx: &'a crate::tcx::Tcx,
    pub opt: relax_codegen::opt::OptCfg,
    pub tier1: bool,
    pub global_allocator: Option<DefId>,
    pub provided: Vec<(u32, u64)>,
}
impl<'a> OpenFrom<'a> {
    pub fn of(u: &'a Unit) -> Self {
        Self::with(u, &u.prog, &u.tcx)
    }
    /// `u`'s settings with program `prog` and types `tcx` (the same program: a settle rebuild's
    /// new state)
    pub fn with(u: &Unit, prog: &'a crate::program::Program, tcx: &'a crate::tcx::Tcx) -> Self {
        OpenFrom { prog, tcx, opt: u.opt.clone(), tier1: u.tier1.is_some(), global_allocator: crate::jit::global_allocator_static_of(prog),
            provided: u.provided.iter().map(|(k, pc)| (*k, pc.binding)).collect() }
    }
}

impl Cache {
    /// Opens the cache for this unit: the directory for this Relax build and target, every
    /// pack's index (packs whose recorded late macro inputs changed are skipped).
    pub fn open(u: &Unit, mode: Mode) -> Result<Cache, String> {
        Self::open_collected(u, mode, None)
    }

    /// `open` under cache root `root` (tests pass their own root; nothing reads it from the
    /// environment afterwards).
    pub fn open_in(u: &Unit, mode: Mode, root: PathBuf) -> Result<Cache, String> {
        Self::open_in_collected(u, mode, root, None)
    }

    pub fn open_collected(u: &Unit, mode: Mode, collected: Option<&CollectedCache>) -> Result<Cache, String> {
        Self::open_in_collected(u, mode, crate::dirs::relax_dir(), collected)
    }

    /// The one opener: cache root `root`, optionally with the restored collected graph.
    pub fn open_in_collected(u: &Unit, mode: Mode, root: PathBuf, collected: Option<&CollectedCache>) -> Result<Cache, String> {
        Self::open_from(&OpenFrom::of(u), mode, root, collected)
    }

    /// The one opener, from what it reads of a unit (`from`): also off the owner thread, for a
    /// program no other thread reaches (settle_rebuild opens the new state's cache before its
    /// replace).
    pub fn open_from(from: &OpenFrom<'_>, mode: Mode, root: PathBuf, collected: Option<&CollectedCache>) -> Result<Cache, String> {
        let runtime = from.prog.session.runtime.clone();
        let options = &from.prog.session.clone().options;
        let verbose = options.verbose;
        let t0 = std::time::Instant::now();
        pack::ensure_private_dir(&root)?;
        let dir = root.join("cache");
        pack::ensure_private_dir(&dir)?;
        let prog = &from.prog;
        // a partial restore's (an edited start): the kept crates' definition tables
        let kept = collected.filter(|c| c.kept.is_some_and(|(k, _)| k <= prog.crates.len() && k <= c.fps.len()));
        let collected = collected.filter(|c| c.kept.is_none() && c.fps.len() <= prog.crates.len()
            && c.defs == prog.defs.len() && c.files == prog.files.len());
        // a partial restore's kept crates: the fingerprints and interfaces of the image (their
        // inputs are the certified ones); only the crates loaded after them are hashed
        let kept_n = kept.and_then(|c| c.kept).map_or(0, |(k, _)| k);
        let fps = match collected {
            Some(c) => c.fps.clone(),
            None => crate::stable::crate_fingerprints_from(prog, kept.map_or(&[][..], |c| &c.fps[..kept_n])),
        };
        // A swap preserves the original cache prefix; runtime crates stay outside boot stores.
        let ncr = fps.len();
        let t_fp = t0.elapsed();
        let mut crate_ident = Vec::with_capacity(ncr);
        let mut crate_by_ident = HashMap::new();
        for c in 0..ncr {
            let mut h = Sip128::new();
            h.str("crate-ident");
            h.str(prog.syms.str(prog.crates[c].name));
            let root_file = prog.mods.get(prog.crates[c].root_mod as usize).map(|m| m.file);
            h.str(root_file.and_then(|f| prog.file_paths.get(f as usize)).map_or("", |p| p.as_str()));
            let id = h.finish128();
            crate_ident.push(id);
            crate_by_ident.insert(id, c as u32);
        }
        let mut cfgh = Sip128::new();
        cfgh.str("relax-cache-cfg");
        cfgh.u32(ENTRY_VERSION);
        cfgh.u32(STORE_FORMAT);
        cfgh.u128(crate::codegen_id::id());
        let o = &from.opt;
        for b in [o.inline, o.inline_all, o.fold, o.cse, o.sroa, o.licm, o.imm, o.layout] {
            cfgh.u8(b as u8);
        }
        cfgh.u8(from.tier1 as u8);
        // which crates are bound to the loader's rustc build (provided.rs) is in the interface
        // fingerprints (items::apply_bound), so only the code that can reach a bound crate is keyed by it
        cfgh.u128(from.prog.session.options.fingerprint());
        // the static a #[global_allocator] program's allocator shim calls: code inlining a shim
        // caller (core::heap::alloc & co) is certified by crate fingerprints, which do not show
        // whether or which allocator the program has (the static's type is in the shim callers'
        // own keys, code_key_bytes)
        match from.global_allocator {
            Some(s) => {
                cfgh.u8(1);
                cfgh.u128(crate_ident.get(prog.def(s).krate as usize).copied().unwrap_or(0));
                cfgh.str(&prog.def_path(s));
            }
            None => cfgh.u8(0),
        }
        let mut c = Cache {
            verbose,
            mode,
            dir: dir.clone(),
            stores: Vec::new(),
            crate_path: Vec::new(),
            stale: Vec::new(),
            depth: Vec::new(),
            key_crate: HashMap::new(),
            cur_crate: 0,
            index: KeyMap::default(),
            mapped_index: None,
            fps,
            iface: Vec::new(),
            iface_plain: Vec::new(),
            view: Vec::new(),
            deps: None,
            name_memo: HashMap::new(),
            body_names: HashMap::new(),
            digests: Vec::new(),
            def_bodies: HashMap::new(),
            code_path: Vec::new(),
            code_stale: Vec::new(),
            cert_rejected: HashSet::new(),
            verdict_rejected: HashSet::new(),
            slot_crates: HashMap::new(),
            const_stale: HashSet::new(),
            const_live: HashSet::new(),
            const_body_keys: HashMap::new(),
            front_stores_of: Vec::new(),
            front_stores_n: 0,
            enc_defs: None,
            crate_by_ident,
            crate_ident,
            cfg: cfgh.finish128(),
            pending: Vec::new(),
            pending_bytes: 0,
            pending_keys: HashSet::new(),
            recording_pending: false,
            front_reserved: HashSet::new(),
            foreign_canon: None,
            def_maps: HashMap::new(),
            crate_defs: Vec::new(),
            crate_defs_n: 0,
            def_keys: HashMap::new(),
            mapped_definitions: None,
            collected_boot: None,
            collected_committed: false,
            file_ids: HashMap::new(),
            file_ids_n: 0,
            file_hash: HashMap::new(),
            data_addr: HashMap::new(),
            const_copy: HashMap::new(),
            data_key: HashMap::new(),
            data_why: HashMap::new(),
            linked: HashSet::new(),
            missed: HashSet::new(),
            linking: HashSet::new(),
            linking_data: HashSet::new(),
            manifest: Vec::new(),
            manifest_present: 0,
            manifest_missing: 0,
            manifest_uncached: 0,
            manifest_keys: HashMap::new(),
            manifest_key_crates: None,
            manifest_old_keys: Vec::new(),
            manifest_present_cached: 0,
            image: None,
            image_ids: None,
            image_rewrite: true,
            manifest_done: false,
            manifest_len: 0,
            image_slots: 0,
            image_stale: 0,
            rec_tier: HashMap::new(),
            image_bad: false,
            manifest_root: None,
            linked_key: HashMap::new(),
            slot_keys: HashMap::new(),
            key_waits: false,
            last_callees: Vec::new(),
            last_thunks: Vec::new(),
            last_data: Vec::new(),
            last_tier1: None,
            last_o2: false,
            last_base: false,
            base_linked: HashSet::new(),
            fnkey_bytes: HashMap::new(),
            pending_verdicts: HashMap::new(),
            slot_by_bytes: Default::default(),
            closure_fps: HashMap::new(),
            stats: CacheStats::default(),
            why_unencodable: HashMap::new(),
            last_why: "",
        };
        // late inputs of crates this program does not have cannot matter to its entries (every
        // entry's key names the crates it depends on)
        let present: HashSet<String> = prog.crates.iter().map(|ci| prog.syms.str(ci.name).to_string()).collect();
        // each crate's store file: <crate>-<hash of Relax build, target, crate fingerprint>.rcs
        for ci in 0..ncr {
            let name = front_store_name(options, prog.syms.str(prog.crates[ci].name), c.fps[ci]);
            c.crate_path.push(dir.join(name));
        }
        // and its code store: the same name over the interface fingerprint
        let t_if = std::time::Instant::now();
        // a restored collected program: the interfaces its recording run keyed code with (the
        // certified inputs are unchanged); file digests are made when a key needs one (def_body)
        match collected.filter(|c| c.ifaces.len() == ncr && std::env::var_os("MAKEPAD_RUN_CACHE_IFACE").is_none()) {
            Some(cc) => {
                c.digests = vec![None; prog.files.len()];
                c.iface_plain = cc.ifaces.clone();
            }
            None => match kept.filter(|k| k.ifaces.len() >= kept_n && std::env::var_os("MAKEPAD_RUN_CACHE_IFACE").is_none()) {
                // the crates loaded after the kept ones: their files, on several threads (a kept
                // file is digested when a key needs it, def_body)
                Some(kc) => {
                    c.digests = vec![None; prog.files.len()];
                    let want: Vec<usize> = (0..prog.files.len()).filter(|&f| prog.file_crate.get(f).is_some_and(|&k| k as usize >= kept_n)).collect();
                    items::file_digests_of(prog, &want, &mut c.digests);
                    c.iface_plain = items::crate_ifaces_from(prog, &c.fps, &c.digests, &kc.ifaces[..kept_n]);
                    c.iface_plain.truncate(ncr);
                }
                None => {
                    // (every file, on several threads)
                    c.digests = items::file_digests(prog);
                    c.iface_plain = items::crate_ifaces(prog, &c.fps, &c.digests);
                    c.iface_plain.truncate(ncr);
                }
            },
        }
        let provided: Vec<(u32, u64)> = from.provided.clone();
        c.iface = items::apply_bound(prog, &c.iface_plain, &provided);
        // the views (a restored collected program digests no file: its interfaces are restored)
        let mut view = items::crate_views(prog, &c.iface_plain, &deps::atomic_crates(prog, ncr));
        view.truncate(ncr);
        c.view = items::apply_bound(prog, &view, &provided);
        c.stats.iface_ns = t_if.elapsed().as_nanos() as u64;
        // MAKEPAD_RUN_CACHE_IFACE=<file>: every file's interface hash and crate's interface
        // fingerprint (which file made an edit's crate recompile)
        if let Ok(path) = std::env::var("MAKEPAD_RUN_CACHE_IFACE") {
            let mut out = String::new();
            for (ci, v) in c.iface.iter().enumerate() {
                out.push_str(&format!("crate\t{}\t{:032x}\t{:032x}\n", prog.syms.str(prog.crates[ci].name), v, c.view.get(ci).copied().unwrap_or(0)));
            }
            for (i, d) in c.digests.iter().enumerate() {
                if let Some(d) = d {
                    out.push_str(&format!("file\t{}\t{:032x}\n", prog.file_paths.get(i).map_or("", |p| p.as_str()), d.iface));
                }
            }
            let _ = std::fs::write(&path, out);
            // MAKEPAD_RUN_CACHE_IFACE_FILE=<path suffix>: that file's interface text too
            if let Ok(want) = std::env::var("MAKEPAD_RUN_CACHE_IFACE_FILE") {
                for (i, p) in prog.file_paths.iter().enumerate() {
                    if p.ends_with(&want) {
                        let _ = std::fs::write(format!("{}.{}.txt", path, i), items::iface_text(&prog.files[i]));
                    }
                }
            }
        }
        // named by the view: an edit of the crate's items keeps its store (the entries' keys say
        // which code still holds)
        for ci in 0..ncr {
            c.code_path.push(dir.join(code_store_name(options, prog.syms.str(prog.crates[ci].name), c.view[ci])));
        }
        c.depth = crate_depths(prog);
        c.stale = vec![false; ncr];
        c.code_stale = vec![false; ncr];
        // open them (in parallel: ~80 files of each kind, each a header + an index copy)
        let opened: Vec<Option<std::sync::Arc<Store>>> = {
            let all: Vec<PathBuf> = c.crate_path.iter().chain(c.code_path.iter()).cloned().collect();
            let paths = &all;
            let n = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 8);
            let next = std::sync::atomic::AtomicUsize::new(0);
            let mut out: Vec<Option<std::sync::Arc<Store>>> = (0..paths.len()).map(|_| None).collect();
            let parts: Vec<Vec<(usize, Option<std::sync::Arc<Store>>)>> = crate::pool::scope(|sc| {
                let hs: Vec<_> = (0..n)
                    .map(|_| {
                        let next = &next;
                        sc.spawn(move || {
                            let mut v = Vec::new();
                            loop {
                                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let Some(p) = paths.get(k) else { break };
                                let saved = collected.and_then(|g| g.indexes.as_ref()).and_then(|ix| ix.stores.get(k))
                                    .filter(|s| s.path == *p).cloned();
                                v.push((k, saved.or_else(|| Store::open(p).map(std::sync::Arc::new))));
                            }
                            v
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
            });
            for (k, s) in parts.into_iter().flatten() {
                out[k] = s;
            }
            out
        };
        let mut late = HashMap::new();
        let total: usize = opened.iter().flatten().map(|s| s.index.len()).sum();
        let mapped_index = collected.and_then(|g| g.indexes.as_ref());
        if mapped_index.is_none() { c.index.reserve(total); }
        let mut opened = opened;
        let code_opened = opened.split_off(ncr);
        for (ci, st) in opened.into_iter().enumerate() {
            let Some(st) = st else { continue };
            // A whole-crate image rejected on its own account (its dependencies were restored:
            // module probes or the image itself no longer valid under the same fingerprint) must
            // be replaced, including obsolete part images (their front keys would keep the
            // fallback parse from repairing them); its code goes too (the parse can differ). A
            // crate parsed only because a dependency was keeps its store: same sources, same
            // parser options (both in the store name), same parse.
            let restored = |k: usize| prog.front_restored.get(k).copied().unwrap_or(false);
            let deps_restored = prog.crates[ci].deps.iter().all(|&(_, k)| restored(k as usize))
                && (0..prog.crates.len()).filter(|&k| matches!(prog.syms.str(prog.crates[k].name), "core" | "alloc" | "std") && k != ci).all(restored);
            if prog.record_front && !restored(ci) && deps_restored && st.index.iter().any(|e| e.kind == KIND_FRONT_CRATE) {
                c.stale[ci] = true;
                c.stats.packs_rejected += 1;
                warm_count::add(warm_count::STORES_REJECTED, 1);
                continue;
            }
            // a changed include!/env! input: the crate's whole file is stale
            if !ledger_current(&st, &present, &mut late) {
                c.stale[ci] = true;
                c.stats.packs_rejected += 1;
                warm_count::add(warm_count::STORES_REJECTED, 1);
                continue;
            }
            let si = c.stores.len() as u32;
            if mapped_index.is_none() { for (e, ix) in st.index.iter().enumerate() {
                if ix.kind == KIND_FRONT || ix.kind == KIND_FRONT_REF || ix.kind == KIND_FRONT_CRATE {
                    c.index.insert(ix.key, (si, e as u32));
                }
            }
            }
            c.stores.push(st);
        }
        // the program's stores by role (its store set, write_sets): keyed by its root crate (the
        // last one loaded)
        if let Some(root) = c.crate_ident.last() {
            let mut roles = std::collections::BTreeMap::new();
            let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for (ci, id) in c.crate_ident.iter().enumerate() {
                if let Some(p) = c.crate_path.get(ci) { roles.insert(format!("front {id:032x}"), name(p)); }
                if let Some(p) = c.code_path.get(ci) { roles.insert(format!("code {id:032x}"), name(p)); }
            }
            if let Some(g) = prog.front_graph.as_ref() {
                roles.insert("graph".to_string(), name(&front_graph_path(&dir, g.identity)));
                roles.insert("collected".to_string(), name(&collected::path(&dir, g.identity)));
            }
            crate::store::note_program(*root as u64, &roles);
        }
        // other Relax builds' files and the cap: pruned off the start's critical path, after
        // this run's stores are open (so marked live: store::live)
        if mode != Mode::ReadOnly {
            let r = root.clone();
            { let _k = crate::zone_alloc::kept();
            let _ = std::thread::Builder::new().name("run-cache-prune".to_string()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                crate::jit::lower_this_thread_priority(&runtime);
                let (b, a) = prune_root(&r);
                if verbose && std::env::var_os("MAKEPAD_RUN_CACHE_TRACE").is_some() && b != a {
                    eprintln!("makepad-run cache: pruned {} -> {} MB", b >> 20, a >> 20);
                }
            });
            }
        }
        // A rejected store changes ordinal positions: use the ordinary index in that case.
        // (front stores only: they come first in `stores`, as the mapped table numbers them)
        if let Some(mapped) = mapped_index {
            if c.stores.len() == mapped.stores.len() {
                c.mapped_index = Some(mapped.clone());
            } else {
                for (si, st) in c.stores.iter().enumerate() {
                    for (ei, e) in st.index.iter().enumerate() {
                        if e.kind == KIND_FRONT || e.kind == KIND_FRONT_REF || e.kind == KIND_FRONT_CRATE { c.index.insert(e.key, (si as u32, ei as u32)); }
                    }
                }
            }
        }
        c.stats.packs = c.stores.len() as u64;
        // the code stores, after the front stores: the ordinary index
        for (ci, st) in code_opened.into_iter().enumerate() {
            let Some(st) = st else { continue };
            // a changed include!/env! input: the crate's code is stale
            if !ledger_current(&st, &present, &mut late) {
                c.code_stale[ci] = true;
                continue;
            }
            let si = c.stores.len() as u32;
            for (e, ix) in st.index.iter().enumerate() {
                if ix.kind != KIND_LEDGER && ix.kind != KIND_MANIFEST && ix.kind != KIND_FRONT && ix.kind != KIND_FRONT_REF && ix.kind != KIND_FRONT_CRATE {
                    c.index.insert(ix.key, (si, e as u32));
                }
            }
            c.stores.push(st);
            c.stats.code_stores += 1;
            warm_count::add(warm_count::STORES, 1);
        }
        c.stats.index_entries = (c.index.len() + c.mapped_index.as_ref().map_or(0, |m| m.len())) as u64;
        let t_packs = t0.elapsed();
        // Load-time front writes make a cold cache nonempty, but need no code definition maps.
        if c.has_code_entries() && !collected.is_some_and(|graph| c.restore_collected_definitions(&from.prog, graph.definitions)) {
            match kept.and_then(|cc| cc.kept.map(|(k, d)| (cc, k, d))) {
                // the kept crates' tables mapped, the others' maps built
                Some((cc, k, d)) if c.restore_kept_definitions(&from.prog, cc.definitions, cc.defs, cc.fps.len(), k, d) => c.build_def_maps_from(from.prog, from.tcx, k),
                _ => c.build_def_maps(from.prog, from.tcx),
            }
        }
        c.stats.open_ns = t0.elapsed().as_nanos() as u64;
        if crate::preload::trace() {
            if let Some(index) = &c.mapped_index {
                eprintln!("makepad-run collected: mapped cache tables: {} stores, {} entries; definition tables {}",
                    index.stores.len(), index.len(), if c.mapped_definitions.is_some() { "mapped" } else { "rebuilt" });
            }
        }
        if verbose && std::env::var_os("MAKEPAD_RUN_CACHE_TRACE").is_some() {
            eprintln!("makepad-run cache: open phases: fingerprints {:.1} ms (interfaces {:.1} ms), stores+index {:.1} ms ({} code stores), def maps {:.1} ms", t_fp.as_secs_f64() * 1e3, c.stats.iface_ns as f64 / 1e6, (t_packs - t_fp).as_secs_f64() * 1e3 - c.stats.iface_ns as f64 / 1e6, c.stats.code_stores, (t0.elapsed() - t_packs).as_secs_f64() * 1e3);
        }
        Ok(c)
    }

    fn front_key(path: &str, source_hash: u128) -> u128 {
        let mut h = Sip128::new();
        h.str("front");
        h.u32(crate::image::VERSION);
        h.str(path);
        h.u128(source_hash);
        h.finish128()
    }

    fn front_part_key(path: &str, source_hash: u128) -> u128 {
        let mut h = Sip128::new();
        h.str("front-part");
        h.u32(crate::image::VERSION);
        h.str(path);
        h.u128(source_hash);
        h.finish128()
    }

    pub fn has_code_entries(&self) -> bool {
        self.stores.iter().any(|s| s.index.iter().any(|e| e.kind == KIND_CODE))
    }

    /// Image availability is local to the crate's store, even when another world already
    /// mapped the same source (the process-wide key index may point at that other store).
    fn has_front_key_in_crate(&mut self, krate: u32, key: u128) -> bool {
        // (the crate's opened stores, found once: every mid-run write asks for every file)
        if self.front_stores_of.len() != self.crate_path.len() || self.front_stores_n != self.stores.len() {
            self.front_stores_n = self.stores.len();
            let mut of = vec![Vec::new(); self.crate_path.len()];
            for (si, st) in self.stores.iter().enumerate() {
                if let Some(k) = self.crate_path.iter().position(|p| *p == st.path) { of[k].push(si as u32); }
            }
            self.front_stores_of = of;
        }
        let stored = self.front_stores_of.get(krate as usize).is_some_and(|v| v.iter().any(|&si| {
            let st = &self.stores[si as usize];
            st.index.binary_search_by_key(&key, |e| e.key).ok().is_some_and(|i| st.index[i].kind == KIND_FRONT_REF)
        }));
        stored || self.pending_keys.contains(&key) && self.pending.iter().any(|(c, e)| *c == krate && e.key == key && e.kind == KIND_FRONT_REF)
    }

    /// Queues references to file `path`'s images (`main`, `part`: where they are) for crate
    /// `krate`'s store unless it has them or they are queued; the number queued. Per crate, as
    /// record_front_crates and the crate images read them: a file two crates load (a lib's and
    /// its bin's `mod x;`) needs a reference in each store, also when the lazy selection loaded
    /// one copy after boot (restore_collected reads the file's own crate's store). Only the
    /// program's crates as the cache opened: run-time modules (sandboxed or not) never write
    /// into the program's stores.
    pub fn push_front_refs(&mut self, krate: u32, path: &str, source_hash: u128, main: Option<&crate::parser::ImageLoc>, part: Option<&crate::parser::ImageLoc>) -> usize {
        if self.mode == Mode::ReadOnly || krate as usize >= self.crate_path.len() {
            return 0;
        }
        let mut n = 0;
        for (key, loc) in [(Self::front_key(path, source_hash), main), (Self::front_part_key(path, source_hash), part)] {
            let Some(loc) = loc else { continue };
            if self.front_reserved.contains(&(krate, key)) { continue; }
            // (found or queued now: the next write's scan takes the memo)
            self.front_reserved.insert((krate, key));
            if self.has_front_key_in_crate(krate, key) { continue; }
            self.pending_keys.insert(key);
            self.push_pending(krate, NewEntry { key, kind: KIND_FRONT_REF, payload: front_run::encode_ref(loc) });
            n += 1;
        }
        n
    }

    /// Warm-session diagnostics (`makepad inspect cache`): counts a fresh build of slot `id` (`base`: one of
    /// warm_count's RETIER_*_OVER) by the tier of its entry in the opened stores.
    pub(crate) fn note_retier_over(&mut self, u: &Unit, id: u32, base: usize) {
        if !self.has_code_entries() { return; }
        let key = self.slot_key(u, id);
        let rank = key.and_then(|k| self.entry_tier(k)).map_or(0, |t| tier_rank(t) as usize + 1);
        warm_count::add(base + rank, 1);
        if trace_on() { trace_keyed(u, id, &format!("fresh-{}-over:{}", if base == warm_count::RETIER_O3_OVER { "O3" } else { "O2" }, ["none", "base", "O1", "O2", "O3"][rank]), key); }
    }

    fn index_at(&self, key: u128) -> Option<(u32, u32)> {
        self.index.get(&key).copied().or_else(|| self.mapped_index.as_ref()?.get(key))
    }
    fn has_index(&self, key: &u128) -> bool { self.index_at(*key).is_some() }

    /// Whether slot `id` has a stored code entry (any tier): it ran in an earlier session.
    pub(crate) fn has_stored_entry(&mut self, u: &Unit, id: u32) -> bool {
        self.has_code_entries() && self.slot_key(u, id).is_some_and(|k| self.entry_tier(k).is_some())
    }

    /// The tier of code entry `key` as stored (its payload's tier byte).
    fn entry_tier(&self, key: u128) -> Option<u8> {
        let (kind, p) = self.get(key)?;
        if kind != KIND_CODE { return None; }
        payload_tier(p)
    }

    fn get(&self, key: u128) -> Option<(u32, &[u8])> {
        let (p, e) = self.index_at(key)?;
        self.stores[p as usize].get_at(e as usize, key)
    }

    /// `get` with the payload checksum verified (payloads linked as code or data).
    fn get_checked(&self, key: u128) -> Option<(u32, &[u8])> {
        let (p, e) = self.index_at(key)?;
        self.stores[p as usize].get_checked_at(e as usize, key)
    }

    /// The crate whose store file an entry keyed by `crates` goes to: the deepest in the
    /// dependency graph (it depends on the others when there is a chain: its version covers
    /// theirs), ties by crate index.
    fn dominant(&self, crates: &[u32]) -> u32 {
        crates.iter().copied().max_by_key(|&c| (self.depth.get(c as usize).copied().unwrap_or(0), c)).unwrap_or(0)
    }

    /// SPEC-V: slot `id`'s body failed the borrow checker. Its code stays out of the cache: a record
    /// not yet written is dropped, later records of it are refused, and an entry that may be on disk
    /// already is replaced by an empty one (newest wins; an empty code payload is a miss), so a
    /// later run never links code the checker refused.
    pub fn reject_verdict(&mut self, u: &Unit, id: u32) {
        if self.mode == Mode::ReadOnly {
            return;
        }
        let Some(key) = self.slot_key(u, id) else { return };
        if !self.verdict_rejected.insert(key) {
            return;
        }
        let before = self.pending.len();
        let mut freed = 0;
        self.pending.retain(|(_, e)| {
            let drop = e.key == key && e.kind == KIND_CODE;
            if drop { freed += e.payload.len() + 32; }
            !drop
        });
        self.pending_bytes -= freed.min(self.pending_bytes);
        let krate = self.key_crate.get(&key).copied().unwrap_or(0);
        if self.verbose {
            eprintln!("makepad-run cache: slot {} {} failed the borrow checker: its code entry is dropped ({} pending)", id, u.fns[id as usize].name, before - self.pending.len());
        }
        self.pending_keys.insert(key);
        self.push_pending(krate, NewEntry { key, kind: KIND_CODE, payload: Vec::new() });
    }

    fn push_pending(&mut self, krate: u32, e: NewEntry) {
        self.pending_bytes += e.payload.len() + 32;
        RECORDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.pending.push((krate, e.into()));
    }

    /// Payload bytes recorded but not yet written to the store files (`RECORDS` counts the
    /// records: a reader that saw no new one need not look again).
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// The entries recorded so far, taken out for writing during the run (on the owner thread;
    /// the write itself, `Spill::write`, runs off the unit). Memory: a cold Stage run held ~200 MB
    /// of recorded code until the exit flush. The keys stay in `pending_keys` (nothing is
    /// recorded twice); the late-input ledger written with them is this run's so far (a later
    /// write or the exit flush replaces it: newest wins per key).
    pub fn spill_take(&mut self) -> Option<Spill> {
        if self.mode == Mode::ReadOnly || self.pending.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending_bytes = 0;
        let marks = front_run::run_marks();
        let mut files = Vec::new();
        for ((c, front), entries) in by_store(pending) {
            let Some((path, stale)) = self.store_of(c, front) else { continue };
            let stale = std::mem::replace(stale, false);
            files.push((path, stale, entries));
        }
        Some(Spill { dir: self.dir.clone(), files, marks })
    }

    // ------------------------------------------------------------ stable identities

    /// A definition's key: its parent's key (the crate's identity at the root), name, kind,
    /// defining file (and, unless its path names it, its offset). Hierarchical, so a crate's
    /// whole map is cheap.
    fn def_key(&mut self, u: &Unit, d: DefId) -> u128 {
        if self.crate_defs_n == u.prog.defs.len() {
            if let Some(k) = self.mapped_definitions.as_ref().and_then(|m| m.key(d)) { return k; }
        }
        if let Some(&k) = self.def_keys.get(&d) {
            return k;
        }
        let mut memo = std::mem::take(&mut self.def_keys);
        let mut files = std::mem::take(&mut self.file_hash);
        let k = def_key_in(&u.prog, &u.tcx, &self.crate_ident, &mut memo, &mut files, d);
        self.def_keys = memo;
        self.file_hash = files;
        k
    }

    /// Every crate's map from definition key to definition, built on several threads (cache
    /// open, before main: nothing changes the program meanwhile). A warm start decodes
    /// definitions of most crates; building their maps lazily on the main thread cost ~30 ms.
    fn build_def_maps(&mut self, prog: &crate::program::Program, tcx: &crate::tcx::Tcx) {
        self.build_def_maps_from(prog, tcx, 0);
    }

    /// build_def_maps of the crates from `first` on (the ones before have mapped tables).
    fn build_def_maps_from(&mut self, prog: &crate::program::Program, tcx: &crate::tcx::Tcx, first: usize) {
        struct ProgTcx<'a> { prog: &'a crate::program::Program, tcx: &'a crate::tcx::Tcx }
        let u = &ProgTcx { prog, tcx };
        let mut by = vec![Vec::new(); u.prog.crates.len()];
        for i in 0..u.prog.defs.len() {
            let d = DefId(i as u32);
            if let Some(v) = by.get_mut(u.prog.def(d).krate as usize) {
                v.push(d);
            }
        }
        self.crate_defs_n = u.prog.defs.len();
        let mut order: Vec<usize> = (first..by.len()).filter(|c| !by[*c].is_empty()).collect();
        order.sort_by_key(|c| std::cmp::Reverse(by[*c].len()));
        let n = std::thread::available_parallelism().map_or(4, |x| x.get()).clamp(1, 8);
        let next = std::sync::atomic::AtomicUsize::new(0);
        struct Shared<'a>(&'a ProgTcx<'a>);
        unsafe impl Sync for Shared<'_> {}
        let sh = Shared(u);
        let ci = &self.crate_ident;
        let results: Vec<Vec<(u32, HashMap<u128, DefId>, u64, HashMap<DefId, u128>)>> = crate::pool::scope(|sc| {
            let hs: Vec<_> = (0..n)
                .map(|_| {
                    let (sh, by, order, next) = (&sh, &by, &order, &next);
                    sc.spawn(move || {
                        let u = sh.0;
                        let mut out = Vec::new();
                        let mut files = HashMap::new();
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&c) = order.get(k) else { break };
                            let mut memo = HashMap::new();
                            let mut m = HashMap::with_capacity(by[c].len());
                            let mut dup = HashSet::new();
                            for &d in &by[c] {
                                let kk = def_key_in(&u.prog, &u.tcx, ci, &mut memo, &mut files, d);
                                if m.insert(kk, d).is_some() {
                                    dup.insert(kk);
                                }
                            }
                            // two definitions with one key: neither decodes (a miss, never the wrong one)
                            for kk in &dup {
                                m.remove(kk);
                            }
                            out.push((c as u32, m, dup.len() as u64, memo));
                        }
                        out
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        });
        for (c, m, dups, memo) in results.into_iter().flatten() {
            self.stats.def_collisions += dups;
            self.def_maps.insert(c, m);
            self.def_keys.extend(memo);
        }
        // the crates before `first` keep their lists: dec_def builds such a map on its first miss
        for v in by.iter_mut().skip(first) { *v = Vec::new(); }
        self.crate_defs = by;
    }

    /// A file version in an entry: its path and source hash (see file_ids).
    /// A file by path and text. None (nothing written): a version a live edit committed -- its
    /// parse is eager, so its expression and block ids are not those of a boot parse of the same
    /// text, and a key naming them would mean other expressions in a run that booted from it
    /// (a warm Scope ran cached code calling `{closure#175197}`, which was no closure there:
    /// SIGILL). Code that names such a file's closures, sites or types is not cached.
    fn enc_file(&self, u: &Unit, file: u32, e: &mut E) -> Option<()> {
        if u.prog.is_edit_version(file) {
            return None;
        }
        match u.prog.file_paths.get(file as usize) {
            Some(p) => {
                // A package's lib and bin parse one file separately: the same path and text, two
                // crates, and rules that depend on the crate (the live layout of the root crate's
                // enums) make their code differ. The owner is part of the identity.
                let owner = u.prog.file_crate.get(file as usize).and_then(|&c| self.crate_ident.get(c as usize)).copied().unwrap_or(0);
                e.u128(file_ident(p, u.prog.files[file as usize].source_hash(p), owner));
            }
            None => e.u128(0),
        }
        Some(())
    }

    /// Closure `expr`'s identity for a key (closure_text_fp), or the one a decoded key gave it
    /// when this run has not parsed it. None: unknown (the key is not cached).
    fn closure_fp(&mut self, u: &Unit, file: u32, expr: u32) -> Option<u64> {
        match closure_text_fp(u, file, expr) {
            Some(fp) => Some(fp),
            None => self.closure_fps.get(&(file, expr)).copied(),
        }
    }

    /// A decoded key or entry says expression `expr` of `file` is the closure with identity
    /// `fp`: true when this program agrees (or has not parsed it yet and no other key said
    /// otherwise; the build checks it then). False rejects the entry: it was made from another
    /// numbering of the file, and linking it would run or call the wrong code.
    fn closure_fp_agrees(&mut self, u: &Unit, file: u32, expr: u32, fp: u64) -> bool {
        let ok = match closure_text_fp(u, file, expr) {
            Some(have) => have == fp,
            None => *self.closure_fps.entry((file, expr)).or_insert(fp) == fp,
        };
        if !ok {
            self.stats.closure_mismatch += 1;
            if self.verbose && self.stats.closure_mismatch <= 8 {
                eprintln!("makepad-run cache: entry rejected: it names expression {} of {} as a closure this program does not have there (made from another parse of the file)",
                    expr & !crate::tcx::ASYNC_FN_BODY, u.prog.file_paths.get(file as usize).map_or("?", |p| p.as_str()));
            }
        }
        ok
    }

    /// The identity a decoded key gave closure `expr` of `file` before this run parsed it
    /// (closure_fps), for the build's check.
    pub(crate) fn closure_expected_fp(&self, file: u32, expr: u32) -> Option<u64> {
        self.closure_fps.get(&(file, expr)).copied()
    }

    fn dec_file(&mut self, u: &Unit, d: &mut D) -> Option<u32> {
        let ident = d.u128()?;
        self.file_by_ident(u, ident)
    }

    /// The file of this program with identity `ident` (enc_file).
    fn file_by_ident(&mut self, u: &Unit, ident: u128) -> Option<u32> {
        if ident == 0 { return None; }
        if self.file_ids_n != u.prog.file_paths.len() {
            for i in self.file_ids_n..u.prog.file_paths.len() {
                // a live edit's version never: its ids are not a boot parse's (enc_file)
                if u.prog.is_edit_version(i as u32) { continue; }
                let p = &u.prog.file_paths[i];
                // the first file with this content keeps the key (later identical copies are the same text)
                let owner = u.prog.file_crate.get(i).and_then(|&c| self.crate_ident.get(c as usize)).copied().unwrap_or(0);
                self.file_ids.entry(file_ident(p, u.prog.files[i].source_hash(p), owner)).or_insert(i as u32);
            }
            self.file_ids_n = u.prog.file_paths.len();
        }
        self.file_ids.get(&ident).copied()
    }

    fn enc_def(&mut self, u: &Unit, d: DefId, e: &mut E, crates: &mut Vec<u32>) {
        let c = u.prog.def(d).krate;
        crates.push(c);
        if let Some(v) = self.enc_defs.as_mut() { v.push(d); }
        // a crate loaded after the cache opened (a run-time module) has no identity: its keys
        // are refused by the callers (`crates` carries it)
        e.u128(self.crate_ident.get(c as usize).copied().unwrap_or(0));
        let k = self.def_key(u, d);
        e.u128(k);
    }

    fn dec_def(&mut self, u: &Unit, dd: &mut D) -> Option<DefId> {
        let ci = dd.u128()?;
        let k = dd.u128()?;
        let c = *self.crate_by_ident.get(&ci)?;
        // definitions added since the maps were built (a live edit): build them again
        if self.crate_defs_n != u.prog.defs.len() {
            self.crate_defs_n = u.prog.defs.len();
            self.crate_defs.clear();
            self.def_maps.clear();
            self.mapped_definitions = None;
        }
        if let Some(mapped) = &self.mapped_definitions {
            // (a partial restore's tables hold for its kept crates' definitions before its suffix;
            // the rest -- the other crates, a kept crate's on-demand tail -- go by the maps)
            match mapped.definition(c, k) {
                Some(d) => return Some(d),
                None if !mapped.partial() => return None,
                None => {}
            }
        }
        if !self.def_maps.contains_key(&c) {
            let t0 = relax_backend::cpu_ns();
            if self.crate_defs.is_empty() {
                let mut by = vec![Vec::new(); u.prog.crates.len()];
                for i in 0..u.prog.defs.len() {
                    let d = DefId(i as u32);
                    if let Some(v) = by.get_mut(u.prog.def(d).krate as usize) {
                        v.push(d);
                    }
                }
                self.crate_defs = by;
            }
            let defs = std::mem::take(self.crate_defs.get_mut(c as usize)?);
            let mut m = HashMap::with_capacity(defs.len());
            let mut dup = HashSet::new();
            for &d in &defs {
                let kk = self.def_key(u, d);
                if m.insert(kk, d).is_some() {
                    dup.insert(kk);
                }
            }
            // two definitions with one key: neither decodes (a miss, never the wrong one)
            for kk in dup {
                m.remove(&kk);
                self.stats.def_collisions += 1;
            }
            if self.verbose && trace_on() {
                eprintln!("makepad-run cache: def map of crate {} ({} defs) in {:.2} ms", u.prog.syms.str(u.prog.crates[c as usize].name), m.len(), (relax_backend::cpu_ns() - t0) as f64 / 1e6);
            }
            self.def_maps.insert(c, m);
        }
        self.def_maps.get(&c)?.get(&k).copied()
    }

    /// A monomorphic type by structure. Inference variables, params, etc. cannot be stored.
    fn enc_ty(&mut self, u: &Unit, t: TyId, e: &mut E, crates: &mut Vec<u32>, depth: u32) -> Option<()> {
        if depth > 64 {
            return None;
        }
        let k = u.tcx.tys.kind(t).clone();
        match k {
            TyKind::Bool => e.u8(1),
            TyKind::Char => e.u8(2),
            TyKind::Str => e.u8(3),
            TyKind::Int(p) => {
                e.u8(4);
                e.u8(prim_code(p));
            }
            TyKind::Float(p) => {
                e.u8(5);
                e.u8(prim_code(p));
            }
            TyKind::Foreign(d) => { e.u8(27); self.enc_def(u, d, e, crates); }
            TyKind::Never => e.u8(6),
            TyKind::Tuple(v) => {
                e.u8(7);
                e.u32(u.tcx.tys.list(v).len() as u32);
                for x in u.tcx.tys.list(v).iter().copied() {
                    self.enc_ty(u, x, e, crates, depth + 1)?;
                }
            }
            TyKind::Array(el, n) => {
                e.u8(8);
                self.enc_ty(u, el, e, crates, depth + 1)?;
                self.enc_const(u, n, e, crates, depth + 1)?;
            }
            TyKind::Slice(x) => {
                e.u8(9);
                return self.enc_ty(u, x, e, crates, depth + 1);
            }
            TyKind::Ref(m, x, r) => {
                e.u8(10);
                e.u8(m as u8);
                self.enc_region(u, r, e)?;
                return self.enc_ty(u, x, e, crates, depth + 1);
            }
            TyKind::Ptr(m, x) => {
                e.u8(11);
                e.u8(m as u8);
                return self.enc_ty(u, x, e, crates, depth + 1);
            }
            TyKind::Adt(d, args, regions) => {
                e.u8(12);
                self.enc_def(u, d, e, crates);
                self.enc_regions(u, regions, e)?;
                return self.enc_tys(u, u.tcx.tys.list(args), e, crates, depth);
            }
            TyKind::FnPtr(ps, r, c, n) => {
                e.u8(13);
                self.enc_tys(u, u.tcx.tys.list(ps), e, crates, depth)?;
                self.enc_ty(u, r, e, crates, depth + 1)?;
                e.u8(c.safety as u8);
                e.u8(c.abi as u8);
                e.u8(c.c_variadic as u8);
                e.u32(n);
            }
            TyKind::FnDef(d, args, regions) => {
                e.u8(14);
                self.enc_def(u, d, e, crates);
                self.enc_regions(u, regions, e)?;
                return self.enc_tys(u, u.tcx.tys.list(args), e, crates, depth);
            }
            TyKind::Closure(file, expr, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: state }) => {
                e.u8(17);
                crates.push(u.prog.file_crate.get(file as usize).copied().unwrap_or(0));
                self.enc_file(u, file, e)?;
                e.u32(expr);
                e.u64(self.closure_fp(u, file, expr)?);
                self.enc_ty(u, closure_kind_ty, e, crates, depth + 1)?; self.enc_ty(u, sig, e, crates, depth + 1)?; self.enc_ty(u, state, e, crates, depth + 1)?;
                // The kind and tupled upvars are the type's own args (ClosureArgs).
                return self.enc_ty(u, owner, e, crates, depth + 1);
            }
            TyKind::Alias(rigid, alias) => {
                e.u8(18); e.u8(rigid as u8);
                let tag = match alias.kind { AliasTyKind::Projection { .. } => 0, AliasTyKind::Inherent { .. } => 1,
                    AliasTyKind::Opaque { .. } => 2, AliasTyKind::Free { .. } => 3 };
                e.u8(tag); self.enc_def(u, alias.kind.def_id(), e, crates);
                self.enc_regions(u, alias.regions, e)?;
                return self.enc_tys(u, u.tcx.tys.list(alias.args), e, crates, depth);
            }
            TyKind::Dyn(d, args, binds, regions, r, bound, autos) => {
                e.u8(19);
                self.enc_def(u, d, e, crates);
                self.enc_regions(u, regions, e)?;
                self.enc_region(u, r, e)?;
                e.u32(bound as u32);
                self.enc_tys(u, u.tcx.tys.list(args), e, crates, depth)?;
                e.u32(u.tcx.tys.list(binds).len() as u32);
                for (bd, bt, n) in u.tcx.tys.list(binds).iter().copied() {
                    e.u32(n);
                    self.enc_def(u, bd, e, crates);
                    self.enc_ty(u, bt, e, crates, depth + 1)?;
                }
                e.u32(u.tcx.tys.list(autos).len() as u32);
                for ad in u.tcx.tys.list(autos).iter().copied() { self.enc_def(u, ad, e, crates); }
            }
            TyKind::Const(v) => {
                e.u8(20);
                return self.enc_const(u, v, e, crates, depth + 1);
            }
            TyKind::Param(_) | TyKind::Infer(_) | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Error | TyKind::Bound(..) | TyKind::Placeholder(_) => return None,
        }
        Some(())
    }

    // rustc_type_ir/src/const_kind.rs:21: persist const structure and stable defs.
    fn enc_const(&mut self, u: &Unit, ct: relax_types::consts::Const, e: &mut E, crates: &mut Vec<u32>, depth: u32) -> Option<()> {
        use relax_types::consts::{ConstKind, AliasConstKind, GenericArg, ValTreeKind};
        if depth > 64 { return None; }
        match u.tcx.tys.const_kind(ct) {
            ConstKind::Value(value) => {
                e.u8(0);
                self.enc_ty(u, value.ty, e, crates, depth + 1)?;
                match value.valtree.kind() {
                    ValTreeKind::Leaf(scalar) => { e.u8(0); e.u8(scalar.size()); e.u128(scalar.to_bits(scalar.size())); }
                    ValTreeKind::Branch(fields) => {
                        e.u8(1); e.u32(fields.len() as u32);
                        for &ct in &**fields { self.enc_const(u, ct, e, crates, depth + 1)?; }
                    }
                }
            }
            ConstKind::Alias(rigid, alias) => {
                e.u8(1); e.u8(rigid as u8);
                e.u8(match alias.kind { AliasConstKind::Projection { .. } => 0, AliasConstKind::InherentSelf { .. } => 1,
                    AliasConstKind::InherentImpl { .. } => 2, AliasConstKind::Free { .. } => 3, AliasConstKind::Anon { .. } => 4 });
                self.enc_def(u, alias.kind.def_id(), e, crates);
                e.u32(alias.args.len() as u32);
                for arg in &*alias.args {
                    match *arg {
                        GenericArg::Type(ty) => { e.u8(0); self.enc_ty(u, ty, e, crates, depth + 1)?; }
                        GenericArg::Const(ct) => { e.u8(1); self.enc_const(u, ct, e, crates, depth + 1)?; }
                        GenericArg::Lifetime(r) => {
                            e.u8(2);
                            self.enc_region(u, r, e)?;
                        }
                    }
                }
            }
            ConstKind::Param(p) => { e.u8(2); e.u32(p.index); }
            ConstKind::Bound(d, b) => { e.u8(3); e.u32(d); e.u32(b.var.0); }
            ConstKind::Infer(_) | ConstKind::Placeholder(_) | ConstKind::Error => return None,
        }
        Some(())
    }

    fn dec_const(&mut self, u: &Unit, d: &mut D, depth: u32) -> Option<relax_types::consts::Const> {
        use relax_types::consts::{ConstKind, AliasConstKind, AliasConst, GenericArg, IsRigid, ValTree, ScalarInt, Value, ParamConst, BoundConst};
        if depth > 64 { return None; }
        let tys = &u.tcx.tys;
        let kind = match d.u8()? {
            0 => {
                let ty = self.dec_ty(u, d, depth + 1)?;
                let valtree = match d.u8()? {
                    0 => { let size = d.u8()?; if size == 0 || size > 16 { return None; }
                        ValTree::from_scalar_int(tys, ScalarInt::try_from_uint(d.u128()?, size)?) }
                    1 => {
                        let count = d.u32()? as usize;
                        if count > d.b.len().saturating_sub(d.p) { return None; }
                        let mut fields = Vec::with_capacity(count);
                        for _ in 0..count { fields.push(self.dec_const(u, d, depth + 1)?); }
                        ValTree::from_branches(tys, &fields)
                    }
                    _ => return None,
                };
                ConstKind::Value(Value { ty, valtree })
            }
            1 => {
                let rigid = match d.u8()? { 0 => IsRigid::Yes, 1 => IsRigid::No, _ => return None };
                let tag = d.u8()?;
                let def_id = self.dec_def(u, d)?;
                let kind = match tag { 0 => AliasConstKind::Projection { def_id }, 1 => AliasConstKind::InherentSelf { def_id },
                    2 => AliasConstKind::InherentImpl { def_id }, 3 => AliasConstKind::Free { def_id }, 4 => AliasConstKind::Anon { def_id }, _ => return None };
                let count = d.u32()? as usize;
                        if count > d.b.len().saturating_sub(d.p) { return None; }
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(match d.u8()? {
                        0 => GenericArg::Type(self.dec_ty(u, d, depth + 1)?),
                        1 => GenericArg::Const(self.dec_const(u, d, depth + 1)?),
                        2 => GenericArg::Lifetime(self.dec_region(u, d)?),
                        _ => return None,
                    });
                }
                ConstKind::Alias(rigid, AliasConst { kind, args: args.into() })
            }
            2 => ConstKind::Param(ParamConst { index: d.u32()? }),
            3 => ConstKind::Bound(d.u32()?, BoundConst { var: crate::types::BoundVar(d.u32()?) }),
            _ => return None,
        };
        Some(tys.intern_const(kind))
    }

    fn enc_tys(&mut self, u: &Unit, v: &[TyId], e: &mut E, crates: &mut Vec<u32>, depth: u32) -> Option<()> {
        e.u32(v.len() as u32);
        v.iter().try_for_each(|x| self.enc_ty(u, *x, e, crates, depth + 1))
    }

    // Lead round-27 design: preserve Binder arity and ReBound identity, as
    // rustc_type_ir/src/binder.rs:31-60 and region_kind.rs:149-158 @8d1a764.
    // erase_regions keeps these regions because they affect selection and TypeId.
    fn enc_region(&self, u: &Unit, r: crate::types::RegionId, e: &mut E) -> Option<()> {
        match u.tcx.tys.region_kind(r) {
            crate::types::RegionKind::ReErased => e.u8(0),
            crate::types::RegionKind::ReBound(db, var) => { e.u8(1); e.u32(db); e.u32(var.0); }
            _ => return None,
        }
        Some(())
    }

    fn dec_region(&self, u: &Unit, d: &mut D) -> Option<crate::types::RegionId> {
        match d.u8()? {
            0 => Some(crate::types::RegionId::ERASED),
            1 => Some(u.tcx.tys.intern_region(crate::types::RegionKind::ReBound(d.u32()?, crate::types::BoundVar(d.u32()?)))),
            _ => None,
        }
    }

    fn enc_regions(&self, u: &Unit, regions: crate::types::RegionArgsId, e: &mut E) -> Option<()> {
        let regions = u.tcx.tys.region_args(regions);
        e.u32(regions.len() as u32);
        for &r in &*regions {
            self.enc_region(u, r, e)?;
        }
        Some(())
    }

    fn dec_regions(&self, u: &Unit, d: &mut D) -> Option<crate::types::RegionArgsId> {
        let count = d.u32()? as usize;
        if count > d.b.len().saturating_sub(d.p) { return None; }
        let mut regions = Vec::with_capacity(count);
        for _ in 0..count { regions.push(self.dec_region(u, d)?); }
        Some(u.tcx.tys.intern_region_args(&regions))
    }

    fn dec_ty(&mut self, u: &Unit, d: &mut D, depth: u32) -> Option<TyId> {
        if depth > 64 {
            return None;
        }
        let tys = &u.tcx.tys;
        let k = match d.u8()? {
            1 => TyKind::Bool,
            2 => TyKind::Char,
            3 => TyKind::Str,
            4 => TyKind::Int(prim_of(d.u8()?)?),
            5 => TyKind::Float(prim_of(d.u8()?)?),
            6 => TyKind::Never,
            7 => TyKind::Tuple(u.tcx.tys.mk_list(&self.dec_tys(u, d, depth)?)),
            8 => {
                let el = self.dec_ty(u, d, depth + 1)?;
                let n = self.dec_const(u, d, depth + 1)?;
                TyKind::Array(el, n)
            }
            9 => TyKind::Slice(self.dec_ty(u, d, depth + 1)?),
            10 => {
                let m = d.u8()? != 0;
                let r = self.dec_region(u, d)?;
                TyKind::Ref(m, self.dec_ty(u, d, depth + 1)?, r)
            }
            11 => {
                let m = d.u8()? != 0;
                TyKind::Ptr(m, self.dec_ty(u, d, depth + 1)?)
            }
            12 => {
                let def = self.dec_def(u, d)?;
                let regions = self.dec_regions(u, d)?;
                TyKind::Adt(def, u.tcx.tys.mk_list(&self.dec_tys(u, d, depth)?), regions)
            }
            13 => {
                let ps = self.dec_tys(u, d, depth)?;
                let r = self.dec_ty(u, d, depth + 1)?;
                TyKind::FnPtr(tys.mk_list(&ps), r, crate::types::FnHeader {
                    safety: match d.u8()? { 0 => crate::types::Safety::Safe, 1 => crate::types::Safety::Unsafe, _ => return None },
                    abi: crate::types::Abi::from_byte(d.u8()?)?,
                    c_variadic: match d.u8()? { 0 => false, 1 => true, _ => return None },
                }, d.u32()?)
            }
            14 => {
                let def = self.dec_def(u, d)?;
                let regions = self.dec_regions(u, d)?;
                TyKind::FnDef(def, u.tcx.tys.mk_list(&self.dec_tys(u, d, depth)?), regions)
            }
            17 => {
                let file = self.dec_file(u, d)?;
                let expr = d.u32()?;
                if !self.closure_fp_agrees(u, file, expr, d.u64()?) { return None; }
                let kind = self.dec_ty(u, d, depth + 1)?;
                let sig = self.dec_ty(u, d, depth + 1)?;
                let state = self.dec_ty(u, d, depth + 1)?;
                let owner = self.dec_ty(u, d, depth + 1)?;
                TyKind::Closure(file, expr, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: kind, closure_sig_as_fn_ptr_ty: sig, tupled_upvars_ty: state })
            }
            18 => {
                let rigid = match d.u8()? { 0 => crate::consts::IsRigid::Yes, 1 => crate::consts::IsRigid::No, _ => return None };
                let tag = d.u8()?;
                let def_id = self.dec_def(u, d)?;
                let kind = match tag {
                    0 => AliasTyKind::Projection { def_id }, 1 => AliasTyKind::Inherent { def_id },
                    2 => AliasTyKind::Opaque { def_id: def_id },
                    3 => AliasTyKind::Free { def_id }, _ => return None,
                };
                {
                    let regions = self.dec_regions(u, d)?;
                    TyKind::Alias(rigid, AliasTy { kind, args: u.tcx.tys.mk_list(&self.dec_tys(u, d, depth)?), regions })
                }
            }
            19 => {
                let def = self.dec_def(u, d)?;
                let regions = self.dec_regions(u, d)?;
                let r = self.dec_region(u, d)?;
                let bound = d.u32()?;
                let args = self.dec_tys(u, d, depth)?;
                let n = d.u32()? as usize;
                let mut binds = Vec::with_capacity(n);
                for _ in 0..n {
                    let bound = d.u32()?;
                    let bd = self.dec_def(u, d)?;
                    binds.push((bd, self.dec_ty(u, d, depth + 1)?, bound));
                }
                let n = d.u32()? as usize;
                let mut autos = Vec::with_capacity(n);
                for _ in 0..n { autos.push(self.dec_def(u, d)?); }
                autos.sort_by_key(|d| d.0);
                TyKind::Dyn(def, u.tcx.tys.mk_list(&args), u.tcx.tys.mk_list(&binds), regions, r, u16::try_from(bound).ok()?, u.tcx.tys.mk_list(&autos))
            }
            20 => TyKind::Const(self.dec_const(u, d, depth + 1)?),
            27 => TyKind::Foreign(self.dec_def(u, d)?),
            _ => return None,
        };
        Some(tys.intern(k))
    }

    fn dec_tys(&mut self, u: &Unit, d: &mut D, depth: u32) -> Option<Vec<TyId>> {
        let n = d.u32()? as usize;
        let mut v = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            v.push(self.dec_ty(u, d, depth + 1)?);
        }
        Some(v)
    }

    fn enc_fnkey(&mut self, u: &Unit, key: &FnKey, e: &mut E, crates: &mut Vec<u32>, depth: u32) -> Option<()> {
        if depth > 32 {
            return None;
        }
        match key.clone() {
            // twins are never cached (their code depends on the sandbox layout of this run)
            FnKey::Shim(shim) => {
                use relax_types::instance::{ShimKind, ReifyReason};
                e.u8(3);
                let def = shim.kind.def_id();
                let (tag, ty) = match shim.kind {
                    ShimKind::VTable(_) => (0, None),
                    ShimKind::Reify(_, _) => (1, None),
                    ShimKind::FnPtr(_, ty) => (2, Some(ty)),
                    ShimKind::ClosureOnce { .. } => (3, None),
                    ShimKind::FnPtrAsPtr(_, ty) => (4, Some(ty)),
                    ShimKind::FnPtrFromPtr(_, ty) => (5, Some(ty)),
                    ShimKind::Clone(..) | ShimKind::DropGlue(..) => unreachable!("type glue has its own key"),
                };
                e.u8(tag);
                self.enc_def(u, def, e, crates);
                if let Some(ty) = ty { self.enc_ty(u, ty, e, crates, 0)?; }
                match shim.kind {
                    ShimKind::ClosureOnce { closure, track_caller, .. } => {
                        self.enc_def(u, closure, e, crates);
                        e.u8(u8::from(track_caller));
                    }
                    ShimKind::Reify(_, reason) => e.u8(match reason { None => 0, Some(ReifyReason::FnPtr) => 1, Some(ReifyReason::Vtable) => 2 }),
                    _ => {}
                }
                self.enc_tys(u, &shim.args, e, crates, 0)
            }
            FnKey::Sandbox(_) => None,
            // SPEC-V: an unverified-mode twin is its base's body lowered as unverified code: keyed
            // by the base (the code that ran before its verdict, and the calls to it, are cached)
            FnKey::Plain(base) => {
                if base as usize >= u.fns.len() {
                    return None;
                }
                e.u8(4);
                let bk = u.fns[base as usize].key.clone();
                self.enc_fnkey(u, &bk, e, crates, depth + 1)
            }
            FnKey::Inst(d, args) => {
                e.u8(0);
                self.enc_def(u, d, e, crates);
                self.enc_tys(u, &args, e, crates, 0)
            }
            FnKey::Closure(file, expr, parent) => {
                if parent as usize >= u.fns.len() {
                    return None;
                }
                e.u8(1);
                crates.push(u.prog.file_crate.get(file as usize).copied().unwrap_or(0));
                self.enc_file(u, file, e)?;
                e.u32(expr);
                e.u64(self.closure_fp(u, file, expr)?);
                let pk = u.fns[parent as usize].key.clone();
                self.enc_fnkey(u, &pk, e, crates, depth + 1)
            }
            // a startup image slot whose identity is still encoded (image.rs): no key yet, and
            // none remembered
            FnKey::Glue(crate::jit::GLUE_UNNAMED, _) => {
                self.key_waits = true;
                None
            }
            FnKey::Glue(g, t) => {
                e.u8(2);
                e.u8(g);
                self.enc_ty(u, t, e, crates, 0)
            }
        }
    }

    /// The slot a key encoding names in this run (startup manifest).
    pub fn slot_for_bytes(&mut self, u: &mut Unit, b: &[u8]) -> Option<u32> {
        self.slot_of_bytes(u, b)
    }

    /// The key encodings of the slots real-time threads of the program rooted at crate `root`
    /// reached in earlier runs (jit.rs realtime; resolve them with slot_for_bytes_unit): compiled
    /// ahead when the program marks its real-time code.
    pub fn rt_set_keys(&self, u: &Unit, root: u32) -> Vec<Vec<u8>> {
        let Ok(b) = std::fs::read(self.rt_set_path(u, root)) else { return Vec::new() };
        let mut d = D { b: &b, p: 0 };
        let mut out = Vec::new();
        while let Some(k) = d.bytes() {
            out.push(k.to_vec());
        }
        out
    }

    /// Writes the real-time set (see rt_set_keys).
    pub fn rt_set_store(&mut self, u: &Unit, root: u32, ids: &[u32]) {
        if self.mode == Mode::ReadOnly {
            return;
        }
        let mut e = E::default();
        let mut n = 0;
        for &id in ids {
            if let Some(b) = self.slot_bytes(u, id) {
                e.bytes(&b);
                n += 1;
            }
        }
        if std::env::var_os("MAKEPAD_RUN_RT_TRACE").is_some() {
            eprintln!("makepad-run rt: recorded {} of {} slots", n, ids.len());
        }
        let path = self.rt_set_path(u, root);
        let tmp = path.with_extension("new");
        if std::fs::write(&tmp, &e.0).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn rt_set_path(&self, u: &Unit, root: u32) -> PathBuf {
        let name = u.prog.crates.get(root as usize).map_or("program", |c| u.prog.syms.str(c.name));
        self.dir.join(format!("rt-{}.keys", name))
    }

    /// memory report (MAKEPAD_RUN_MEM_REPORT): the recorder's and linker's tables (capacity based)
    pub(crate) fn mem_lines(&self) -> Vec<(String, u64)> {
        fn map<K, V, S>(name: &str, m: &HashMap<K, V, S>, deep: impl Fn(&V) -> usize) -> (String, u64) {
            let slot = std::mem::size_of::<(K, V)>() + 1;
            (format!("cache {} ({})", name, m.len()), (m.capacity() * slot + m.values().map(deep).sum::<usize>()) as u64)
        }
        fn set<K, S>(name: &str, m: &HashSet<K, S>) -> (String, u64) {
            (format!("cache {} ({})", name, m.len()), (m.capacity() * (std::mem::size_of::<K>() + 1)) as u64)
        }
        vec![
            (format!("cache pending entries ({}; payload bytes)", self.pending.len()), self.pending_bytes as u64),
            map("key_crate", &self.key_crate, |_| 0),
            (format!("cache index ({})", self.index.len()), (self.index.capacity() * 41) as u64),
            map("slot_crates", &self.slot_crates, |v| v.capacity() * 4),
            map("def_bodies", &self.def_bodies, |_| 0),
            map("def_maps", &self.def_maps, |m| m.capacity() * 21),
            (format!("cache crate_defs ({})", self.crate_defs_n), self.crate_defs.iter().map(|v| v.capacity() * 4).sum::<usize>() as u64),
            map("def_keys", &self.def_keys, |_| 0),
            map("file_ids", &self.file_ids, |_| 0),
            map("file_hash", &self.file_hash, |_| 0),
            map("data_addr", &self.data_addr, |_| 0),
            map("data_key", &self.data_key, |_| 0),
            set("linked", &self.linked), set("missed", &self.missed),
            map("linked_key", &self.linked_key, |_| 0),
            map("slot_keys", &self.slot_keys, |_| 0),
            map("fnkey_bytes", &self.fnkey_bytes, |v| v.as_ref().map_or(0, |b| b.capacity())),
            map("slot_by_bytes", &self.slot_by_bytes, |_| 0),
            (format!("cache slot_by_bytes key bytes"), self.slot_by_bytes.keys().map(|k| k.len()).sum::<usize>() as u64),
            map("pending_verdicts", &self.pending_verdicts, |_| 0),
            map("closure_fps", &self.closure_fps, |_| 0),
            (format!("cache manifest ({})", self.manifest.len()), self.manifest.iter().map(|m| m.capacity() + 24).sum::<usize>() as u64),
        ]
    }

    /// Slot `id`'s key encoding (memo).
    fn slot_bytes(&mut self, u: &Unit, id: u32) -> Option<Vec<u8>> {
        if let Some(b) = self.fnkey_bytes.get(&id) {
            return b.clone();
        }
        // a slot the startup image made: the key it was made from (image.rs)
        if let Some(b) = self.image_ids.as_ref().and_then(|i| i.bytes_of(id)) {
            return Some(b.to_vec());
        }
        // The exit manifest uses only identities already encoded by a lookup/record.
        // enc_fnkey -> enc_ty -> capture_query can type-check a closure's owner and
        // lazily parse/lower its body, after the exiting thread's TLS is gone.
        if crate::preempt::is_stopped() && !self.recording_pending { return None; }
        let key = u.fns[id as usize].key.clone();
        let mut e = E::default();
        let mut cr = Vec::new();
        let outer = std::mem::replace(&mut self.key_waits, false);
        let b = self.enc_fnkey(u, &key, &mut e, &mut cr, 0)
            // As for code_key_bytes: run-time crates have no boot identity. Do not
            // leave their unresolvable slots in the startup manifest either.
            .filter(|()| cr.iter().all(|&c| (c as usize) < self.crate_ident.len())).map(|()| e.0);
        if !self.key_waits { self.fnkey_bytes.insert(id, b.clone()); }
        self.key_waits |= outer;
        b
    }

    /// The slot an encoded key names in this run (memo by the bytes).
    fn slot_of_bytes(&mut self, u: &mut Unit, b: &[u8]) -> Option<u32> {
        unsafe { Linker::new(self).slot_of_bytes(u, b) }
    }

    /// Whether slot `id` has an entry (speculative linking only follows cached callees).
    pub fn has(&mut self, u: &Unit, id: u32) -> bool {
        match self.slot_key(u, id) {
            Some(k) => self.has_index(&k),
            None => false,
        }
    }

    /// makepad inspect (`versions`): the cache key of slot `id`'s code entry.
    pub(crate) fn inspect_key(&mut self, u: &Unit, id: u32) -> Option<u128> {
        self.slot_key(u, id)
    }

    /// The cache key of slot `id`'s O1 code, and the function's identity bytes.
    fn slot_key(&mut self, u: &Unit, id: u32) -> Option<u128> {
        if let Some(k) = self.slot_keys.get(&id) {
            return *k;
        }
        // build_image also comes here: no capture queries or body digests at exit.
        // Leave an unknown key for the next run's ordinary lookup / mid-run write.
        if crate::preempt::is_stopped() && !self.recording_pending { return None; }
        let outer = std::mem::replace(&mut self.key_waits, false);
        let k = self.slot_key_uncached(u, id);
        if !self.key_waits {
            // the identity bytes too: the exit's manifest and image name a slot only by bytes
            // already encoded (slot_bytes), and a warm run's slots are encoded here, by their
            // lookup, never by a record -- without this, its manifest kept only what it compiled
            if let Some((_, bytes)) = &k {
                self.fnkey_bytes.entry(id).or_insert_with(|| Some(bytes.clone()));
            }
            self.slot_keys.insert(id, k.as_ref().map(|(k, _)| *k));
        }
        self.key_waits |= outer;
        k.map(|(k, _)| k)
    }

    /// Slot `id`'s code key and its identity bytes (`enc_fnkey`, as `slot_bytes` encodes them).
    fn slot_key_uncached(&mut self, u: &Unit, id: u32) -> Option<(u128, Vec<u8>)> {
        let key = u.fns[id as usize].key.clone();
        let (k, crates, bytes) = self.code_key_bytes(u, &key)?;
        self.slot_crates.insert(id, crates);
        Some((k, bytes))
    }

    /// Whether the slot identity `b` (enc_fnkey bytes) names only crates unchanged since the
    /// loaded manifest was recorded (`manifest_key_crates`): its code key is then the recorded
    /// one, as for every slot of those crates (a closure, named by its file: no).
    pub(super) fn identity_unchanged(&self, b: &[u8]) -> bool {
        if self.manifest_keys.is_empty() || !crate::reload::edited_crates().is_empty() { return false; }
        let Some(ok) = &self.manifest_key_crates else { return true };
        let mut all = true;
        let mut d = D { b, p: 0 };
        let read = image::skip_fnkey(&mut d, 0, &mut |a| all &= match a {
            image::Atom::Crate(ident) => self.crate_by_ident.get(&ident).is_some_and(|&c| ok.get(c as usize) == Some(&true)),
            image::Atom::File(_) => false,
        });
        read.is_some() && d.p == b.len() && all
    }

    fn code_key_bytes(&mut self, u: &Unit, key: &FnKey) -> Option<(u128, Vec<u32>, Vec<u8>)> {
        let mut e = E::default();
        let mut crates = Vec::new();
        let outer = self.enc_defs.replace(Vec::new());
        let r = self.enc_fnkey(u, key, &mut e, &mut crates, 0);
        // the allocator shim's callers call the program's #[global_allocator] (or the default):
        // their code depends on it, which the fn's own identity does not show
        let mut shim = E::default();
        if key_defs(u, key).first().is_some_and(|&d| u.calls_allocator_shim(d)) {
            match u.global_allocator() {
                Some((s, t)) => {
                    shim.u8(1);
                    self.enc_def(u, s, &mut shim, &mut crates);
                    self.enc_ty(u, t, &mut shim, &mut crates, 0)?;
                }
                None => shim.u8(0),
            }
        }
        let named = std::mem::replace(&mut self.enc_defs, outer).unwrap_or_default();
        r?;
        // code of (or instantiated with) a crate loaded after the cache opened: never cached
        if crates.iter().any(|&c| c as usize >= self.crate_ident.len()) {
            return None;
        }
        crates.sort_unstable();
        crates.dedup();
        let dom = self.dominant(&crates);
        // a slot of the startup manifest whose crates are as it was recorded from and none
        // edited since: its key as recorded (the bodies below hash to what they hashed to then)
        let unchanged = |c: &u32| self.manifest_key_crates.as_ref().is_none_or(|ok| ok.get(*c as usize) == Some(&true));
        let recorded = if self.manifest_keys.is_empty() || !crate::reload::edited_crates().is_empty() || !crates.iter().all(unchanged) { None }
            else { self.manifest_keys.get(&identity_hash(&e.0)).copied() };
        let k = match recorded {
            Some(k) => k,
            None => {
                let g = self.deps_graph(u);
                let mut h = Sip128::new();
                h.str("code-deps");
                h.u128(self.cfg);
                h.write(&e.0);
                h.write(&shim.0);
                // what the crates give besides their items (deps.rs): name, cfg, dependencies,
                // bound crates, the whole fingerprints of the sysroot, and the items every node of
                // the crate sees (inner attributes, glob imports)
                for &c in &crates {
                    h.u128(*self.view.get(c as usize)?);
                    h.u128(g.always(c));
                }
                // the items the identity names (the definition, the types of its arguments): what
                // they reach of the program's interfaces; one with no node (an item a fn body
                // defines) by its crate's whole interface fingerprint
                let mut defs = named.clone();
                defs.sort_unstable_by_key(|d| d.0);
                defs.dedup();
                for d in defs {
                    match g.def_closure(&u.prog, d) {
                        Some(x) => h.u128(x),
                        None if g.in_graph(&u.prog, d) => {
                            h.str("coarse");
                            h.u128(*self.iface.get(u.prog.def(d).krate as usize)?);
                        }
                        None => {}
                    }
                }
                // the bodies the code is made from: the definition's, a closure's enclosing fns'
                // -- their text and position, and what the names in them find
                let kd = key_defs(u, key);
                for &d in &kd {
                    h.u128(self.def_body(u, d));
                    h.u128(self.body_names_of(u, d, &g));
                }
                // types defined in a fn body (their text is that body's)
                self.local_bodies(u, &named, &mut h);
                let k0 = h.finish128();
                // and what the names of their macro expansions find (unknown until the body is
                // parsed or an earlier run recorded them: no key yet, none remembered)
                let mut h = Sip128::new();
                h.str("code-exp");
                h.u128(k0);
                for &d in &kd {
                    match self.exp_closure(u, d, k0, dom, &g) {
                        Some(x) => h.u128(x),
                        None => {
                            self.key_waits = true;
                            return None;
                        }
                    }
                }
                h.finish128()
            }
        };
        self.key_crate.insert(k, dom);
        Some((k, crates, e.0))
    }

    /// The item graph (cache/deps.rs), on the first need: the stored one when it was built from
    /// this program's very sources (a warm start maps it), else built over it -- the rows of the
    /// crates whose sources and dependencies are as it was built from are taken over, only the
    /// other crates' boot files are digested and walked (a restored collected program digests
    /// files lazily) -- and recorded for the next start. MAKEPAD_RUN_DEPS_VERIFY=1: a graph built
    /// over a stored one is checked against one built from scratch.
    fn deps_graph(&mut self, u: &Unit) -> std::sync::Arc<deps::Deps> {
        if let Some(g) = &self.deps {
            return g.clone();
        }
        let t0 = std::time::Instant::now();
        let prog = &u.prog;
        let ncr = self.iface.len();
        let valid = deps::Deps::valid(prog, self.cfg, &self.fps);
        let root = self.manifest_root.unwrap_or(ncr.saturating_sub(1) as u32);
        let key = self.deps_key(root);
        let stored = key.and_then(|k| {
            let (si, _) = self.index_at(k)?;
            let store = self.stores.get(si as usize)?.clone();
            let (kind, p) = self.get_checked(k)?;
            if kind != KIND_DEPS { return None; }
            // SAFETY: `store` (kept by the graph) holds the mapping
            let p: &'static [u8] = unsafe { std::slice::from_raw_parts(p.as_ptr(), p.len()) };
            deps::Deps::read(prog, ncr, p, store)
        });
        if stored.as_ref().is_some_and(|g| g.built_from() == valid && g.crate_count() == prog.crates.len()) {
            let g = std::sync::Arc::new(stored.unwrap());
            if self.verbose {
                eprintln!("makepad-run cache: item graph: {} nodes mapped in {:.1} ms", g.nodes_n, t0.elapsed().as_secs_f64() * 1e3);
            }
            self.deps = Some(g.clone());
            return g;
        }
        if self.digests.len() < prog.files.len() {
            self.digests.resize(prog.files.len(), None);
        }
        let atomic = deps::atomic_crates(prog, ncr);
        let digest = |clean: &[bool], digests: &mut Vec<Option<std::sync::Arc<items::FileDigest>>>| {
            let want: Vec<usize> = (0..prog.boot_files.min(prog.files.len()))
                .filter(|&f| digests[f].is_none() && prog.file_crate.get(f).is_some_and(|&c| !atomic.get(c as usize).copied().unwrap_or(true) && !clean.get(c as usize).copied().unwrap_or(false)))
                .collect();
            items::file_digests_of(prog, &want, digests);
            want.len()
        };
        // (a graph whose component ids ran far is not built over: the next one starts them again)
        let stored = stored.filter(|g| g.ids_left());
        let clean = deps::Deps::clean_crates(prog, &self.iface_plain, stored.as_ref());
        let mut digested = digest(&clean, &mut self.digests);
        let t_dg = t0.elapsed();
        let built = deps::Deps::build(prog, &self.digests, &self.iface_plain, valid, stored.as_ref());
        let over = built.is_some() && clean.iter().any(|&c| c);
        let g = match built {
            Some(g) => g,
            None => {
                // the alias or macro names changed: every crate is walked
                digested += digest(&[], &mut self.digests);
                deps::Deps::build(prog, &self.digests, &self.iface_plain, valid, None).expect("a graph built from scratch")
            }
        };
        if self.verbose {
            eprintln!("makepad-run cache: item graph: {} nodes ({} walked, {} of {} crates taken over), {} edges, {} coarse, built in {:.1} ms (digests of {} files {:.1} ms; files to {:.1}, edges to {:.1}, components to {:.1}, hashed to {:.1})",
                g.nodes_n, g.walked, if over { clean.iter().filter(|&&c| c).count() } else { 0 }, clean.len(), g.edges, g.coarse, t0.elapsed().as_secs_f64() * 1e3,
                digested, t_dg.as_secs_f64() * 1e3, g.phases[0], g.phases[1], g.phases[2], g.phases[3]);
        }
        if over && std::env::var("MAKEPAD_RUN_DEPS_VERIFY").is_ok_and(|v| v == "1") {
            digest(&[], &mut self.digests);
            let scratch = deps::Deps::build(prog, &self.digests, &self.iface_plain, valid, None).expect("a graph built from scratch");
            match g.same_closures(&scratch) {
                Ok(()) => eprintln!("makepad-run cache: item graph verified: built over the stored graph = built from scratch"),
                Err(e) => eprintln!("makepad-run cache: item graph verify FAILED: {e}"),
            }
        }
        if let Some(k) = key.filter(|_| self.mode != Mode::ReadOnly && (root as usize) < ncr) {
            // (one slot per root: the newest graph replaces the one it was built over)
            self.pending_keys.insert(k);
            self.push_pending(root, NewEntry { key: k, kind: KIND_DEPS, payload: g.encode() });
        }
        let g = std::sync::Arc::new(g);
        self.deps = Some(g.clone());
        g
    }

    /// The key of the stored item graph of the program rooted at crate `root`: one per root and
    /// Relax build (a newer graph replaces it; Deps::valid says what it was built from).
    fn deps_key(&self, root: u32) -> Option<u128> {
        let mut h = Sip128::new();
        h.str("deps-graph-v2");
        h.u128(self.cfg);
        h.u128(*self.crate_ident.get(root as usize)?);
        Some(h.finish128())
    }

    /// File `f`'s digest (made on the first need).
    fn digest_of(&mut self, u: &Unit, f: usize) -> Option<std::sync::Arc<items::FileDigest>> {
        let pf = u.prog.files.get(f)?;
        if self.digests.len() <= f {
            self.digests.resize(u.prog.files.len(), None);
        }
        Some(self.digests[f].get_or_insert_with(|| std::sync::Arc::new(items::file_digest(pf))).clone())
    }

    /// What name `n` finds from crate `c` (memo of deps.rs name_closure).
    fn name_closure(&mut self, g: &deps::Deps, c: u32, n: deps::Name) -> u128 {
        *self.name_memo.entry((c, n)).or_insert_with(|| g.name_closure(c, n))
    }

    /// Definition `d`'s body (when a hole: deps.rs): what the names written in it find.
    fn body_names_of(&mut self, u: &Unit, d: DefId, g: &deps::Deps) -> u128 {
        if let Some(&k) = self.body_names.get(&d) {
            return k;
        }
        let def = u.prog.def(d);
        let (f, item, c) = (def.file as usize, def.item, def.krate);
        let mut h = Sip128::new();
        h.str("body-names");
        if let Some(dg) = self.digest_of(u, f) {
            if let Some((lo, hi)) = dg.body_span(item) {
                let (mut names, mut invoked) = (Vec::new(), Vec::new());
                deps::scan(&u.prog.files[f].src, &[(lo, hi)], &[], &mut names, &mut invoked);
                names.sort_unstable();
                names.dedup();
                let mut v: Vec<u128> = names.iter().map(|&n| self.name_closure(g, c, n)).filter(|&x| x != 0).collect();
                v.sort_unstable();
                v.dedup();
                h.u32(v.len() as u32);
                for x in v { h.u128(x); }
            }
        }
        let k = h.finish128();
        self.body_names.insert(d, k);
        k
    }

    /// What the names of the macro expansions in definition `d`'s body find (0: its body invokes
    /// no macro that can name anything; deps.rs). The names come from the body's parse in this run
    /// (then recorded under `k0`, the code key without them, for a later run) or from that record.
    /// None: the body is not parsed and nothing was recorded (the key is not known yet).
    fn exp_closure(&mut self, u: &Unit, d: DefId, k0: u128, dom: u32, g: &deps::Deps) -> Option<u128> {
        let def = u.prog.def(d);
        let (f, item, c) = (def.file as usize, def.item, def.krate);
        let Some(dg) = self.digest_of(u, f) else { return Some(0) };
        let Some((lo, hi)) = dg.body_span(item) else { return Some(0) };
        let pf = &u.prog.files[f];
        let (mut names, mut invoked) = (Vec::new(), Vec::new());
        deps::scan(&pf.src, &[(lo, hi)], &[], &mut names, &mut invoked);
        if !invoked.iter().any(|&m| g.loud(m)) {
            return Some(0);
        }
        let mut kh = Sip128::new();
        kh.str("exp-names");
        kh.u128(k0);
        kh.u128(self.def_key(u, d));
        let ekey = kh.finish128();
        let owner = dg.hole_owner(item);
        let lazy = matches!(pf.ast.items.get(owner as usize).map(|it| &it.kind), Some(crate::ast::ItemKind::Fn(_, None, _)));
        let names = match deps::exp_names(pf, lo, hi, lazy) {
            Some(n) => {
                if self.mode != Mode::ReadOnly && !self.has_index(&ekey) && self.pending_keys.insert(ekey) {
                    let mut p = Vec::with_capacity(4 + n.len() * 8);
                    p.extend_from_slice(&(n.len() as u32).to_le_bytes());
                    for x in &n { p.extend_from_slice(&x.to_le_bytes()); }
                    self.push_pending(dom, NewEntry { key: ekey, kind: KIND_EXP_NAMES, payload: p });
                }
                n
            }
            None => match self.get_checked(ekey) {
                Some((KIND_EXP_NAMES, p)) => {
                    let n = u32::from_le_bytes(p.get(0..4)?.try_into().ok()?) as usize;
                    if p.len() != 4 + n * 8 { return None; }
                    (0..n).map(|i| u64::from_le_bytes(p[4 + i * 8..12 + i * 8].try_into().unwrap())).collect()
                }
                _ => return None,
            },
        };
        let mut v: Vec<u128> = names.iter().map(|&n| self.name_closure(g, c, n)).filter(|&x| x != 0).collect();
        v.sort_unstable();
        v.dedup();
        let mut h = Sip128::new();
        h.str("exp-closure");
        h.u32(v.len() as u32);
        for x in v { h.u128(x); }
        Some(h.finish128())
    }

    /// The bodies that define any of `named` (local items), into a key.
    fn local_bodies(&mut self, u: &Unit, named: &[DefId], h: &mut Sip128) {
        let mut seen: Vec<u128> = Vec::new();
        for &d in named {
            let def = u.prog.def(d);
            let f = def.file as usize;
            let Some(pf) = u.prog.files.get(f) else { continue };
            if self.digests.len() <= f {
                self.digests.resize(u.prog.files.len(), None);
            }
            let dg = self.digests[f].get_or_insert_with(|| std::sync::Arc::new(items::file_digest(pf))).clone();
            if let Some(b) = items::nested_body_hash(pf, &dg, def.item) {
                if !seen.contains(&b) {
                    seen.push(b);
                    h.u128(b);
                }
            }
        }
    }

    /// Definition `d`'s own part of the keys of its code: its body (when outside its crate's
    /// interface) and where its code reports itself (cache/items.rs).
    fn def_body(&mut self, u: &Unit, d: DefId) -> u128 {
        if let Some(&k) = self.def_bodies.get(&d) {
            return k;
        }
        let t0 = relax_backend::cpu_ns();
        let def = u.prog.def(d);
        let f = def.file as usize;
        let mut h = Sip128::new();
        h.str("def-body");
        if let Some(pf) = u.prog.files.get(f) {
            if self.digests.len() <= f {
                self.digests.resize(u.prog.files.len(), None);
            }
            // a file loaded after the cache opened (a run-time module) gets its digest now
            let dg = self.digests[f].get_or_insert_with(|| std::sync::Arc::new(items::file_digest(pf))).clone();
            match items::body_hash(pf, &dg, def.item) {
                Some(b) => { h.u8(1); h.u128(b); }
                None => h.u8(0),
            }
            h.u128(items::pos_digest(pf, &dg, def.item));
        }
        let k = h.finish128();
        self.def_bodies.insert(d, k);
        self.stats.body_ns += relax_backend::cpu_ns() - t0;
        k
    }

    /// borrowck lane: the verdict key of slot `id`: metadata of its tier-0 code entry, keyed by
    /// that entry's key (the fn key, the interface fingerprints of its crates -- each covers its
    /// dependencies, so a callee's signature or field view -- and its own body) and the checker
    /// version. It goes to the code entry's store.
    pub fn borrowck_key(&mut self, u: &Unit, id: u32) -> Option<u128> {
        let code = self.slot_key(u, id)?;
        let mut h = Sip128::new();
        h.str("borrowck");
        h.u32(BORROWCK_VERSION);
        h.u128(code);
        let k = h.finish128();
        let dom = self.key_crate.get(&code).copied().unwrap_or(0);
        self.key_crate.insert(k, dom);
        Some(k)
    }

    /// The key of the run's coherence verdict (orphan rules, overlapping impls: Tcx::report_coherence):
    /// the checker version, the cache configuration (Relax build, options), every crate's
    /// interface fingerprint (its items outside fn bodies, cfg, dependencies) and the files the
    /// program's impls come from (the selection loads a subset of files). A clean verdict under this key is stored
    /// as a borrowck-kind entry with an empty payload. None after a live edit.
    pub fn coherence_key(&self, prog: &crate::program::Program) -> Option<u128> {
        if !crate::reload::edited_crates().is_empty() || self.iface_plain.len() != prog.crates.len() { return None; }
        let mut h = Sip128::new();
        h.str("coherence-verdict");
        h.u32(COHERENCE_VERSION);
        h.u128(self.cfg);
        // by the crates' interfaces (their impls and every item they can name; a fn body is
        // interface when it declares an impl), not their whole texts: a body edit checks nothing
        h.u32(self.iface_plain.len() as u32);
        for i in &self.iface_plain { h.u128(*i); }
        let mut from = vec![false; prog.files.len()];
        for im in &prog.impls {
            *from.get_mut(prog.def(im.def).file as usize)? = true;
        }
        h.u32(prog.impls.len() as u32);
        for (i, f) in from.iter().enumerate() {
            if *f { h.str(prog.file_paths.get(i).map_or("", |p| p.as_str())); }
        }
        Some(h.finish128())
    }

    /// The verdict stored for this key (on disk, or recorded by this run).
    pub fn borrowck_stored(&self, key: u128) -> Option<StoredVerdict> {
        match self.get(key) {
            Some((KIND_BORROWCK, p)) => dec_verdict(p),
            _ => self.pending_verdicts.get(&key).cloned(),
        }
    }

    pub fn owner_verdicts(&self, epoch: u64) -> OwnerVerdicts {
        let store = self.code_path.first().and_then(|p| self.stores.iter().find(|st| st.path == *p)).cloned();
        OwnerVerdicts::new(self.cfg, self.fps.clone(), self.iface.clone(), epoch, store)
    }

    /// Records a completed verdict (written with the next flush).
    pub fn record_borrowck(&mut self, key: u128, v: StoredVerdict) {
        if self.pending_keys.insert(key) {
            let kc = self.key_crate.get(&key).copied().unwrap_or(0);
            self.push_pending(kc, NewEntry { key, kind: KIND_BORROWCK, payload: enc_verdict(&v) });
            self.pending_verdicts.insert(key, v);
        }
    }

    // ------------------------------------------------------------ recording

    fn why(&mut self, w: &'static str) {
        *self.why_unencodable.entry(w).or_insert(0) += 1;
        self.last_why = w;
    }

    /// Encodes one symbol + offset as a stable reference; None when it cannot be stored.
    /// `rw`: the writable blocks the entry named so far (their `Syms::rw_blocks` indices): a
    /// block symbol is encoded with its position in it, so a decode that links the entry's symbols
    /// in the same order gives every reloc to one block (a slice iterator's pointer and its end
    /// pointer) the one block, not a fresh block per reloc.
    fn enc_sym(&mut self, u: &Unit, s: Sym, e: &mut E, rw: &mut Vec<u32>) -> Option<()> {
        let Some(k) = u.syms.kind(s.id) else {
            self.why("symbol without a kind");
            return None;
        };
        match k {
            SymKind::Rt(n) => {
                e.u8(1);
                e.str(n);
            }
            SymKind::Data(i) => {
                let dk = match self.data_entry(u, i) {
                    Some(k) => k,
                    None => {
                        // data_entry named why (its own, or a pointer word's target's)
                        if self.last_why.is_empty() || self.last_why == "unrecorded reason" {
                            self.why("data object");
                        }
                        return None;
                    }
                };
                e.u8(2);
                e.u128(dk);
            }
            SymKind::Static(d) => {
                e.u8(3);
                let mut cr = Vec::new();
                self.enc_def(u, d, e, &mut cr);
            }
            SymKind::Foreign(_) => {
                // by its definition: the address comes from the same lookup a fresh compile makes
                let Some(d) = foreign_def_of(u, s) else {
                    self.why("foreign symbol without a definition");
                    return None;
                };
                let d = self.canon_foreign(u, d);
                e.u8(4);
                let mut cr = Vec::new();
                self.enc_def(u, d, e, &mut cr);
            }
            SymKind::Missing(_) => {
                self.why("missing foreign fn");
                return None;
            }
            // a static's domain-0 address (asm! sym): code naming one is not cached
            SymKind::StaticAddr(_) => {
                self.why("asm sym of a static");
                return None;
            }
            SymKind::Site(i) => {
                let sites = u.sites.lock().unwrap_or_else(|x| x.into_inner());
                let st = sites.get(i as usize)?;
                e.u8(6);
                self.enc_file(u, st.file, e)?;
                e.u32(st.pos);
                e.str(&st.msg);
            }
            SymKind::Thunk(id) => {
                e.u8(7);
                let key = u.fns[id as usize].key.clone();
                let mut cr = Vec::new();
                if self.enc_fnkey(u, &key, e, &mut cr, 0).is_none() {
                    self.why("thunk key");
                    return None;
                }
            }
            SymKind::TierRec(id) => {
                e.u8(12);
                let key = u.fns[id as usize].key.clone();
                let mut cr = Vec::new();
                if self.enc_fnkey(u, &key, e, &mut cr, 0).is_none() {
                    self.why("loop counter key");
                    return None;
                }
            }
            SymKind::Rw(k) => {
                // a constant's per-domain copy holds the value, so it is named as the copy of the
                // constant's data object (tag 10: the link copies the linked object again), never
                // as a fresh block (tag 8: zeroed, for storage code writes before it reads)
                let copied = u.syms.const_copies.lock().unwrap_or_else(|x| x.into_inner()).get(&s.id).copied();
                if let Some(orig) = copied {
                    let i = match u.syms.data_at(orig).and_then(|d| u.syms.kind(d.id)) {
                        Some(SymKind::Data(i)) => i,
                        _ => {
                            self.why("constant copied per domain");
                            return None;
                        }
                    };
                    let dk = self.data_entry(u, i)?;
                    e.u8(10);
                    e.u128(dk);
                    e.u64(s.off as u64);
                    return Some(());
                }
                let (sz, al) = u.syms.rw_blocks.lock().unwrap_or_else(|x| x.into_inner()).get(k as usize).copied()?;
                e.u8(8);
                e.u64(sz);
                e.u64(al);
                let ord = rw.iter().position(|b| *b == k).unwrap_or_else(|| {
                    rw.push(k);
                    rw.len() - 1
                });
                e.u32(ord as u32);
            }
            SymKind::Provided(n) => {
                e.u8(9);
                e.str(&n);
            }
            // a `#[thread_local]` static's offset: placed again by the linking run, which must
            // give the code's shape (static TLS or the block, the first-access copy or not)
            SymKind::ThreadLocal(d) => {
                let pl = u.tls_places.get(&d).copied()?;
                e.u8(11);
                e.u8(tls_shape(&pl));
                let mut cr = Vec::new();
                self.enc_def(u, d, e, &mut cr);
            }
        }
        e.u64(s.off as u64);
        Some(())
    }

    /// The content key of data object `idx` (bytes, alignment, pointer words), recording its
    /// entry; None when it cannot be stored (mutable, opaque, unattributed pointers).
    fn data_entry(&mut self, u: &Unit, idx: u32) -> Option<u128> {
        if let Some(&k) = self.data_key.get(&idx) {
            if k.is_none() {
                let w = self.data_why.get(&idx).copied().unwrap_or("data object: cycle");
                self.why(w);
            }
            return k;
        }
        // a cycle through this object (only statics can be cyclic; they are not data objects)
        self.data_key.insert(idx, None);
        let r = (|| {
            let (start, len, align, mutable) = u.syms.obj_info(idx)?;
            if mutable {
                self.why("data object: mutable");
                return None;
            }
            let mut ptrs = match crate::syms::obj_pointer_words(u, idx) {
                Some(p) => p,
                None => {
                    self.why("data object: pointer words");
                    return None;
                }
            };
            ptrs.sort_unstable_by_key(|(o, _)| *o);
            let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, len as usize) }.to_vec();
            let mut e = E::default();
            e.u32(ENTRY_VERSION);
            e.u64(align);
            let name = u.syms.obj_name(idx);
            e.u8(name.is_some() as u8);
            if let Some(n) = &name {
                e.str(n);
            }
            let mut b = bytes;
            let mut end = 0;
            for (o, _) in &ptrs {
                let o = *o as usize;
                if o < end || o + 8 > b.len() {
                    self.why("data object: pointer words");
                    return None;
                }
                b[o..o + 8].fill(0);
                end = o + 8;
            }
            e.bytes(&b);
            e.u32(ptrs.len() as u32);
            let mut rw = Vec::new();
            for (o, t) in &ptrs {
                e.u32(*o);
                self.enc_sym(u, *t, &mut e, &mut rw)?;
            }
            let key = {
                let mut h = Sip128::new();
                h.str("data");
                h.u128(self.cfg);
                h.write(&e.0);
                // a named object (a constant, a vtable, a location) is its own entry: two
                // constants with equal bytes are two objects, as lowering makes them, so code
                // linked from the cache has the constant pool a fresh compile gives it
                // (merged by content, a warm run's fresh builds pooled them as one word)
                if let Some(n) = u.syms.obj_name(idx) {
                    h.str(&n);
                }
                h.finish128()
            };
            if let (Some(w), Some(n)) = (dump_filter().filter(|_| self.verbose), u.syms.obj_name(idx)) {
                if n.contains(w) {
                    let hex: String = e.0.iter().map(|b| format!("{:02x}", b)).collect();
                    let ws: Vec<String> = ptrs.iter().map(|(o, t)| format!("{}:{}+{}", o, crate::syms::describe(u, *t), t.off)).collect();
                    eprintln!("makepad-run cache: DATA {} key {:032x} ptrs {:?} enc {}", n, key, ws, hex);
                }
            }
            if self.mode != Mode::ReadOnly && !self.has_index(&key) && self.pending_keys.insert(key) {
                let kc = self.cur_crate;
                self.push_pending(kc, NewEntry { key, kind: KIND_DATA, payload: e.0 });
            }
            Some(key)
        })();
        if r.is_none() && !self.data_why.contains_key(&idx) {
            self.data_why.insert(idx, self.last_why);
        }
        self.data_key.insert(idx, r);
        r
    }

    /// Records slot `id`'s freshly compiled O1 code (on the owner thread).
    /// `code` is the backend's output (relocated fields zeroed), `relocs` its relocations.
    pub fn record(&mut self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)]) {
        if u.fns[id as usize].unverified.load(std::sync::atomic::Ordering::Relaxed) { warm_count::add(warm_count::REC_UNVERIFIED, 1); }
        self.record_entry(u, id, code, relocs, pc_map, None, RecKind::First, None)
    }

    /// What the install of slot `id` leaves for its entry's encoding later (the main thread's
    /// installs before the first present: key and certificate encoding is the cost). The code is
    /// the backend's unrelocated output, which installing patches in place; the nounwind inputs,
    /// the tier and the dependency slots can change or be reclaimed before the encoding runs.
    /// None: `record` would store nothing.
    pub(crate) fn defer_record(&self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)]) -> Option<PendingRecord> {
        if self.mode == Mode::ReadOnly || u.fns[id as usize].patch_gen != 0 || self.linked.contains(&id) {
            return None;
        }
        if u.fns[id as usize].unverified.load(std::sync::atomic::Ordering::Relaxed) { warm_count::add(warm_count::REC_UNVERIFIED, 1); }
        let rir = u.fns[id as usize].rir.as_ref();
        Some(PendingRecord {
            id,
            code: code.to_vec(),
            relocs: relocs.to_vec(),
            pc_map: pc_map.to_vec(),
            nounwind: rir.map_or_else(Vec::new, |r| r.nounwind_inputs.clone()),
            baseline: rir.is_some_and(|r| r.baseline),
            deps: record_deps(u, id, &[]),
            epoch: crate::jit::patch_epoch(),
            o3: None,
        })
    }

    /// `record_o3`'s inputs for an encoding later (the O3 install is a publish: the record's key and
    /// certificate encoding runs in the bounded record batches instead). None: it would store nothing.
    pub(crate) fn defer_record_o3(&self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)], inlined: &[u32], nounwind: &[(u32, bool)]) -> Option<PendingRecord> {
        if self.mode == Mode::ReadOnly || u.fns[id as usize].patch_gen != 0 {
            return None;
        }
        let extra: Vec<u32> = inlined.iter().copied().chain(nounwind.iter().filter(|x| x.1).map(|x| x.0)).collect();
        Some(PendingRecord {
            id,
            code: code.to_vec(),
            relocs: relocs.to_vec(),
            pc_map: pc_map.to_vec(),
            nounwind: nounwind.to_vec(),
            baseline: false,
            deps: record_deps(u, id, &extra),
            epoch: crate::jit::patch_epoch(),
            o3: Some((inlined.to_vec(), nounwind.to_vec())),
        })
    }

    /// Encodes a deferred record: `record`, from the install-time inputs.
    pub(crate) fn record_pending(&mut self, u: &Unit, p: &PendingRecord) {
        if !pending_current(u, p) {
            // a source changed (or the slot was patched) since the install: its key would name
            // the new text for old code. A lost entry is harmless; a stale one is not.
            if p.o3.is_some() { warm_count::add(warm_count::O3_STALE, 1); }
            return;
        }
        let previous = std::mem::replace(&mut self.recording_pending, true);
        let o3 = p.o3.as_ref().map(|(i, n)| (i.as_slice(), n.as_slice()));
        self.record_entry(u, p.id, &p.code, &p.relocs, &p.pc_map, o3, RecKind::First, Some(p));
        self.recording_pending = previous;
    }

    /// Records the O2 code of slot `id` (the O2 worker's full build: a `#[relax(O2)]` slot, or a
    /// slot found hot) as its ordinary entry, replacing the O1 entry it may have, so a warm
    /// launch links O2 code at the first call. A replaced entry's code may be in the startup
    /// image: the manifest is written again.
    pub fn record_o2(&mut self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)]) {
        // The O2 worker can finish after O3 installed (or linked from the cache). Never
        // downgrade its entry, even when that O3 build itself was too large to cache.
        if u.fns[id as usize].o3.load(std::sync::atomic::Ordering::Relaxed) { return; }
        self.record_entry(u, id, code, relocs, pc_map, None, RecKind::O2, None)
    }

    /// Records verified O1 code of slot `id` (jit.rs o1v: a slot whose SPEC-V verdict came clean
    /// after its unverified -- baseline -- code was built, or a slot only ever inlined into O3
    /// code) as its entry, replacing an entry below O1 (baseline), never one at O1 or above:
    /// the cache keeps the session's final verified code. False: nothing was recorded.
    pub fn record_o1_verified(&mut self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)]) -> bool {
        if u.fns[id as usize].o3.load(std::sync::atomic::Ordering::Relaxed) { return false; }
        let before = warm_count::get(warm_count::O1V_WRITTEN);
        self.record_entry(u, id, code, relocs, pc_map, None, RecKind::O1Up, None);
        warm_count::get(warm_count::O1V_WRITTEN) != before
    }

    /// The tier rank (baseline 0, O1 1, O2 2, O3 3) of slot `id`'s newest entry, recorded this run
    /// or stored: None when the slot has no storable key, Some(None) when it has no entry.
    pub(crate) fn slot_entry_rank(&mut self, u: &Unit, id: u32) -> Option<Option<u8>> {
        let key = self.slot_key(u, id)?;
        if self.cert_rejected.contains(&key) && !self.pending_keys.contains(&key) { return Some(None); }
        Some(self.rec_tier.get(&key).copied().or_else(|| self.entry_tier(key)).map(tier_rank))
    }

    #[allow(clippy::too_many_arguments)]
    fn record_entry(&mut self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)], o3: Option<(&[u32], &[(u32, bool)])>, kind: RecKind, pending: Option<&PendingRecord>) {
        let _mem = crate::memstat::tag(crate::memstat::Tag::CacheStore);
        let o2 = kind == RecKind::O2;
        // O3 and O2 code replace the slot's entry, verified O1 code a baseline one; O1 code is
        // recorded once
        let replace = o3.is_some() || kind != RecKind::First;
        let f = &u.fns[id as usize];
        if self.mode == Mode::ReadOnly || f.patch_gen != 0 || u.prog.is_edit_version(f.file) || (!replace && self.linked.contains(&id)) {
            trace_lookup(u, id, "norecord:mode/patch/linked");
            if o3.is_some() { warm_count::add(warm_count::O3_DROPPED, 1); }
            return;
        }
        // a foreign fn's slot holds a jump to the C address (no relocation): never stored
        if matches!(u.fns[id as usize].key, FnKey::Inst(d, _) if u.prog.def(d).kind == DefKind::ForeignFn) {
            return;
        }
        let Some(key) = self.slot_key(u, id) else {
            if o3.is_some() { warm_count::add(warm_count::O3_DROPPED, 1); }
            self.stats.unencodable += 1;
            self.why("fn key");
            trace_lookup(u, id, "unstorable:fn key");
            relax_codegen::warm::reason(id, "unstorable:fn key");
            self.remember_outcome(u, id, None, "cold record-rejected:fn key");
            return;
        };
        // a body the borrow checker rejected (reject_verdict): never stored
        if self.verdict_rejected.contains(&key) {
            trace_lookup(u, id, "norecord:verdict failed");
            return;
        }
        // the entry's store file (its data objects go there too)
        self.cur_crate = self.key_crate.get(&key).copied().unwrap_or(0);
        // an entry this run rejected (a slot its code took in changed) is replaced
        let rejected = self.cert_rejected.contains(&key) && !self.pending_keys.contains(&key);
        let existing = (self.has_index(&key) && !rejected) || self.pending_keys.contains(&key);
        if existing && !replace {
            trace_lookup(u, id, "norecord:exists");
            return;
        }
        // a verified O1 rebuild never replaces an entry at O1 or above
        if kind == RecKind::O1Up && existing && self.rec_tier.get(&key).copied().or_else(|| self.entry_tier(key)).is_some_and(|t| tier_rank(t) >= tier_rank(TIER_O1)) {
            trace_lookup(u, id, "norecord:o1v-not-below");
            return;
        }
        let Some(payload) = self.encode_entry(u, id, code, relocs, pc_map, o3, kind, pending) else {
            if o3.is_some() { warm_count::add(warm_count::O3_UNENCODABLE, 1); }
            let w = self.last_why;
            trace_lookup(u, id, &format!("norecord:encode:{}", w));
            if o3.is_none() {
                relax_codegen::warm::reason(id, &format!("unstorable:{}", w));
                self.remember_outcome(u, id, Some(key), &format!("cold record-rejected:{}", w));
            }
            return;
        };
        // the same code again (a hot slot rebuilt by every run): the entry and the startup
        // image that holds it stay as they are, nothing is appended
        if replace && existing && !self.pending_keys.contains(&key) && self.get(key).is_some_and(|(kind, p)| kind == KIND_CODE && p == &payload[..]) {
            if o3.is_some() { warm_count::add(warm_count::O3_SAME, 1); }
            return;
        }
        self.pending_keys.insert(key);
        // a replacement of an entry with no recorded tier, and one that moves its slot's tier up
        // (O1 -> O2/O3), count like a stale image slot (record_manifest: rewritten past 2%; the
        // next start leaves a slot lazy whose entry outranks its image code, img_live, and the
        // image code of an entry replaced at the same tier is code for the same key); an O3
        // payload replacing an O3 entry neither (PGO-driven O3 code differs run to run; every
        // warm run rewrote the whole image)
        if replace && (existing || self.linked.contains(&id)) {
            match self.entry_tier(key) {
                None => self.stats.rewrite_why[0] += 1,
                Some(old) if payload_tier(&payload).is_some_and(|new| tier_rank(new) > tier_rank(old)) => self.stats.rewrite_why[1] += 1,
                Some(_) => {}
            }
        }
        if let Some(t) = payload_tier(&payload) {
            self.rec_tier.insert(key, t);
            if trace_on() { trace_keyed(u, id, &format!("rec:{}", ["O1", "O3", "O2", "base"].get(t as usize).copied().unwrap_or("?")), Some(key)); }
        }
        let kc = self.cur_crate;
        self.push_pending(kc, NewEntry { key, kind: KIND_CODE, payload });
        self.stats.recorded += 1;
        warm_count::add(warm_count::RECORDED, 1);
        if o3.is_some() { warm_count::add(warm_count::O3_WRITTEN, 1); }
        if o2 { warm_count::add(warm_count::O2_WRITTEN, 1); }
        if kind == RecKind::O1Up { warm_count::add(warm_count::O1V_WRITTEN, 1); }
        if o3.is_none() { self.remember_outcome(u, id, Some(key), "recorded"); }
    }

    fn nounwind_bytes(&mut self, u: &Unit, facts: &[(u32, bool)]) -> Option<Vec<u8>> {
        let mut e = E::default();
        let mut nw = Vec::with_capacity(facts.len());
        for &(c, value) in facts {
            let Some(key) = self.slot_bytes(u, c) else {
                self.stats.unencodable += 1;
                self.why("nounwind callee key");
                return None;
            };
            nw.push((key, value));
        }
        nw.sort_unstable();
        e.u32(nw.len() as u32);
        for (key, value) in nw { e.bytes(&key); e.u8(value as u8); }
        Some(e.0)
    }

    /// The dependency certificate of a code: every slot of `deps` (`record_deps`: the slots whose
    /// RIR it took in), by its key encoding with its key
    /// now. The entry links only while each still has that key (its body, position and
    /// interface unchanged). None: one has no key (the entry cannot be stored).
    fn cert_bytes(&mut self, u: &Unit, deps: &[u32]) -> Option<Vec<u8>> {
        let t0 = relax_backend::cpu_ns();
        let r = self.cert_bytes_inner(u, deps);
        self.stats.cert_write_ns += relax_backend::cpu_ns() - t0;
        r
    }

    fn cert_bytes_inner(&mut self, u: &Unit, deps: &[u32]) -> Option<Vec<u8>> {
        let mut items: Vec<(Vec<u8>, u128)> = Vec::new();
        let mut crates: Vec<u32> = Vec::new();
        for &c in deps {
            // A new caller can inline an edited body without itself being a patch. Its
            // cached/memoized key must not certify code built from that live file version.
            let f = &u.fns[c as usize];
            if f.patch_gen != 0 || u.prog.is_edit_version(f.file) { return None; }
            let b = self.slot_bytes(u, c)?;
            let k = self.slot_key(u, c)?;
            crates.extend(self.slot_crates.get(&c).into_iter().flatten().copied());
            items.push((b, k));
        }
        items.sort_unstable();
        items.dedup();
        let mut e = E::default();
        // the fast path: the crates every dependency's key comes from, with a hash of their
        // whole fingerprints now -- while that hash is the same, no source those keys are made
        // of changed, and the list below need not be checked
        let mut idents: Vec<u128> = crates.iter().map(|&c| self.crate_ident.get(c as usize).copied()).collect::<Option<Vec<_>>>()?;
        idents.sort_unstable();
        idents.dedup();
        e.u32(idents.len() as u32);
        for &i in &idents { e.u128(i); }
        e.u128(self.cert_fps(&idents)?);
        e.u32(items.len() as u32);
        for (b, k) in items {
            e.bytes(&b);
            e.u128(k);
        }
        Some(e.0)
    }

    /// A hash of crates `idents`' whole fingerprints (certificate fast path). None: one is
    /// not a crate of this program.
    fn cert_fps(&self, idents: &[u128]) -> Option<u128> {
        let mut h = Sip128::new();
        h.str("cert-fps");
        for i in idents {
            let c = *self.crate_by_ident.get(i)?;
            h.u128(*i);
            h.u128(*self.fps.get(c as usize)?);
        }
        Some(h.finish128())
    }

    /// The payload of a code entry (O1, or O3 with its inlined slots);
    /// None: it cannot be stored (`last_why` says why).
    #[allow(clippy::too_many_arguments)]
    fn encode_entry(&mut self, u: &Unit, id: u32, code: &[u8], relocs: &[RelocAt], pc_map: &[(u32, u64)], o3: Option<(&[u32], &[(u32, bool)])>, kind: RecKind, pending: Option<&PendingRecord>) -> Option<Vec<u8>> {
        let o2 = kind == RecKind::O2;
        // unlinked code only: a pool word must still be zero (linked code holds this run's values)
        for rl in relocs {
            let o = rl.at as usize;
            if rl.kind == RelocKind::Pool64 && code.get(o..o + 8).map_or(true, |w| w.iter().any(|b| *b != 0)) {
                self.stats.unencodable += 1;
                self.why("linked code");
                return None;
            }
        }
        self.last_why = "unrecorded reason";
        if let Some(w) = dump_filter().filter(|_| self.verbose) {
            let name = crate::syms::stable_fn_name(u, id);
            if name.contains(w) {
                for rl in relocs {
                    let t = match &rl.target {
                        Reloc::Sym(s) => format!("{} +{}", crate::syms::describe(u, *s), s.off),
                        Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => format!("slot {}", crate::syms::stable_fn_name(u, *x)),
                        r => format!("{:?}", r),
                    };
                    eprintln!("makepad-run cache: RELOCS rec {} {} {}", name, rl.at, t);
                }
            }
        }
        let mut e = E::default();
        e.u32(ENTRY_VERSION);
        e.str(&u.fns[id as usize].name);
        // the code's level (TIER_O1 / TIER_O3 / TIER_O2 / TIER_BASE); O3: the inlined slots (by key)
        let base = o3.is_none() && kind == RecKind::First && match pending {
            Some(p) => p.baseline,
            None => u.fns[id as usize].rir.as_ref().is_some_and(|r| r.baseline),
        };
        e.u8(if o3.is_some() { TIER_O3 } else if o2 { TIER_O2 } else if base { TIER_BASE } else { TIER_O1 });
        let inl = o3.map_or(&[][..], |t| t.0);
        e.u32(inl.len() as u32);
        for &x in inl {
            match self.slot_bytes(u, x) {
                Some(b) => e.bytes(&b),
                None => {
                    self.stats.unencodable += 1;
                    self.why("inlined slot key");
                    return None;
                }
            }
        }
        // its nounwind inputs: stable keys replace session slots; false answers are included
        // so absence/recursion cannot become an implicit input
        let facts = match (pending, o3) {
            (Some(p), _) => p.nounwind.as_slice(),
            (_, Some(t)) => t.1,
            _ => u.fns[id as usize].rir.as_ref().map_or(&[][..], |r| r.nounwind_inputs.as_slice()),
        };
        e.0.extend_from_slice(&self.nounwind_bytes(u, facts)?);
        // the slots whose RIR this code took in, with their keys (link_entry checks them)
        let extra: Vec<u32> = match o3 {
            Some((inl, nw)) => inl.iter().copied().chain(nw.iter().filter(|x| x.1).map(|x| x.0)).collect(),
            None => Vec::new(),
        };
        // the slots whose RIR this code took in, with their keys (link_entry checks them)
        let deps;
        let deps = match pending {
            Some(p) => &p.deps,
            None => {
                deps = record_deps(u, id, &extra);
                &deps
            }
        };
        let Some(cert) = self.cert_bytes(u, deps) else {
            self.stats.unencodable += 1;
            self.why("dependency key");
            return None;
        };
        e.0.extend_from_slice(&cert);
        e.bytes(code);
        e.u32(relocs.len() as u32);
        let mut rw = Vec::new();
        for rl in relocs {
            e.u32(rl.at);
            e.u8(kind_code(rl.kind));
            let r = &rl.target;
            let ok = match r {
                Reloc::Sym(s) => self.enc_sym(u, *s, &mut e, &mut rw).is_some(),
                Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => {
                    e.u8(match r { Reloc::Slot(_) => 20, Reloc::Call(_) => 25, _ => 21 });
                    match self.slot_bytes(u, *x) {
                        Some(b) => {
                            e.bytes(&b);
                            true
                        }
                        None => {
                            self.why("callee key");
                            false
                        }
                    }
                }
                Reloc::TlsKey => {
                    e.u8(22);
                    true
                }
                Reloc::TlsSlow => {
                    e.u8(23);
                    true
                }
                Reloc::Resume => {
                    e.u8(24);
                    true
                }
                Reloc::Value(_) => {
                    self.why("value reloc");
                    false
                }
            };
            if !ok {
                self.stats.unencodable += 1;
                if trace_on() {
                    let w = match r {
                        Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => format!("slot/thunk key {}", crate::syms::stable_fn_name(u, *x)),
                        Reloc::Sym(s) => format!("{} ({})", self.last_why, crate::syms::describe(u, *s)),
                        _ => self.last_why.to_string(),
                    };
                    trace_lookup(u, id, &format!("unstorable:{}", w));
                }
                return None;
            }
        }
        // the pc map as the jit keeps it (PcMap: varint offset and position deltas), its
        // positions naming this entry's file table by index
        let mut files: Vec<u32> = Vec::new();
        let mut local = Vec::with_capacity(pc_map.len());
        for (o, pos) in pc_map {
            let f = relax_backend::rir::pos_file(*pos);
            let fi = match files.iter().position(|x| *x == f) {
                Some(i) => i,
                None => {
                    files.push(f);
                    files.len() - 1
                }
            };
            // (the inline context in the top bits stays)
            local.push((*o, (fi as u64) << 32 | (*pos & 0xffff_ffff) | (*pos & !relax_backend::rir::pos_plain(u64::MAX))));
        }
        let mut pcb = Vec::new();
        crate::jit::PcMap::encode_to(&mut pcb, local.into_iter());
        e.bytes(&pcb);
        e.u32(files.len() as u32);
        for f in files {
            // a live edit's version: positions unknown to the next run (enc_file)
            if self.enc_file(u, f, &mut e).is_none() {
                e.str("");
                e.u128(0);
            }
        }
        Some(e.0)
    }

    // ------------------------------------------------------------ linking

    #[cfg(test)]
    fn link_data(&mut self, u: &mut Unit, key: u128) -> Option<u64> {
        unsafe { Linker::new(self).link_data(u, key) }
    }

    /// Looks slot `id` up and links it.
    pub fn lookup(&mut self, u: &mut Unit, id: u32) -> Option<LinkedCode> {
        unsafe { Linker::new(self).lookup(u, id) }
    }

    /// This program's startup manifest was loaded: an earlier run of it reached main with this cache.
    /// The loaded startup manifest carries a startup image (prelink's input).
    pub fn has_startup_image(&self) -> bool {
        self.image.is_some()
    }

    pub fn has_startup_manifest(&self) -> bool {
        !self.manifest.is_empty()
    }

    /// The loaded startup manifest describes this cache: every code entry its slots linked is
    /// in the index.
    pub fn startup_manifest_valid(&self) -> bool {
        self.has_startup_manifest() && self.manifest_missing == 0
    }

    // ------------------------------------------------------------ writing

    /// Writes the entries recorded this run as one new pack (with this run's late macro inputs),
    /// then trims the cache to its size cap. Called at exit.
    fn manifest_key(&self, root: u32) -> Option<u128> {
        let mut h = Sip128::new();
        h.str("startup slots");
        h.u128(self.cfg);
        // the program's view, not its interface: the manifest names each slot's key, which says
        // whether its entry holds after an edit (load_manifest)
        h.u128(*self.view.get(root as usize)?);
        Some(h.finish128())
    }

    /// Key of the entry naming the sources a startup manifest's code keys were made from.
    fn manifest_sources_key(&self, manifest: u128) -> u128 {
        let mut h = Sip128::new();
        h.str("startup slots sources");
        h.u128(manifest);
        h.finish128()
    }

    /// Every crate's fingerprint (sources, cfg, dependencies) and the code configuration: equal
    /// in two runs, the code key of every slot is equal too.
    fn sources_fp(&self) -> u128 {
        let mut h = Sip128::new();
        h.str("sources");
        h.u128(self.cfg);
        for &fp in &self.fps {
            h.u128(fp);
        }
        h.finish128()
    }

    /// The settle rebuild's new cache (opened as a warm start opens it, from the same stores) takes
    /// over what the old one learned about this process: which data objects and slots were
    /// linked from which entries (a second link of the same data object must find its address,
    /// not place a copy), the entries it rejected, the counters, and that the collected program
    /// was committed (the exit does not encode it again). The old cache's recorded entries were
    /// written to the stores before this one opened them (`spill_take`).
    pub(crate) fn inherit_run_state(&mut self, old: &Cache) {
        self.data_addr = old.data_addr.clone();
        self.const_copy = old.const_copy.clone();
        self.data_key = old.data_key.clone();
        self.linked = old.linked.clone();
        self.linked_key = old.linked_key.clone();
        self.base_linked = old.base_linked.clone();
        self.cert_rejected = old.cert_rejected.clone();
        self.pending_keys = old.pending_keys.clone();
        self.rec_tier = old.rec_tier.clone();
        self.pending_verdicts = old.pending_verdicts.clone();
        self.manifest_done = old.manifest_done;
        self.manifest_len = old.manifest_len;
        self.collected_committed = true;
        self.stats = old.stats.clone();
        // the same program (settle_rebuild compares it): the same item graph and definitions
        self.deps = old.deps.clone();
        self.name_memo = old.name_memo.clone();
        self.body_names = old.body_names.clone();
    }

    /// Reads the newest startup manifest of the program rooted at crate `root` (and its
    /// prelinked startup image, if it has one).
    pub fn load_manifest(&mut self, root: u32) {
        self.manifest_root = Some(root);
        let Some(key) = self.manifest_key(root) else { return };
        if let Some(pk) = self.stores.iter().find(|st| st.path == self.code_path[root as usize]).cloned() {
            // checksummed before use: its image section becomes executable code (prelink)
            if let Some((KIND_MANIFEST, p)) = pk.get_checked(key) {
                let mut d = D { b: p, p: 0 };
                let mut out = Vec::new();
                let mut keys = Vec::new();
                let (mut missing, mut uncached, mut present_cached) = (0, 0, 0);
                let ok = (|| {
                    if d.u32()? != ENTRY_VERSION {
                        return None;
                    }
                    let n = d.u32()? as usize;
                    let present = d.u32()? as usize;
                    for i in 0..n {
                        out.push(d.bytes()?.to_vec());
                        let k = d.u128()?;
                        keys.push(k);
                        match k {
                            0 => uncached += 1,
                            k if !self.has_index(&k) => missing += 1,
                            _ => if i < present { present_cached += 1 },
                        }
                    }
                    Some(present.min(n))
                })();
                if let Some(present) = ok {
                    // the image section after the slot list (the store keeps the mapping alive)
                    if d.u32() == Some(IMAGE_MAGIC) {
                        let rest = &p[d.p..];
                        self.image = Some(ManifestImage { _store: pk.clone(), at: rest.as_ptr() as usize, len: rest.len() });
                    }
                    // recorded from these very sources: its keys are this run's keys; else those
                    // of the slots whose crates its image records with this run's fingerprints
                    let sources = self.manifest_sources_key(key);
                    let same = pk.get(sources).is_some_and(|(kind, p)| kind == KIND_MANIFEST && D { b: p, p: 0 }.u128() == Some(self.sources_fp()));
                    let crates = if same { Some(None) } else {
                        self.image.as_ref().and_then(|im| image::image_crates(im.bytes())).map(|recorded| {
                            let recorded: HashMap<u128, u128> = recorded.into_iter().collect();
                            Some(self.crate_ident.iter().zip(&self.fps).map(|(id, fp)| recorded.get(id) == Some(fp)).collect())
                        })
                    };
                    if let Some(crates) = crates.filter(|_| missing == 0 && self.mode != Mode::Verify) {
                        self.manifest_keys = out.iter().zip(&keys).filter(|(_, k)| **k != 0).map(|(b, k)| (identity_hash(b), *k)).collect();
                        self.manifest_key_crates = crates;
                    }
                    self.manifest = out;
                    self.manifest_old_keys = keys;
                    self.manifest_present = present;
                    self.manifest_missing = missing;
                    self.manifest_uncached = uncached;
                    self.manifest_present_cached = present_cached;
                    self.image_rewrite = self.image.is_none();
                }
                return;
            }
        }
    }

    /// Records this run's startup manifest (the slots installed in the first seconds after main,
    /// in order, the first `present` of them before the first present; per slot its stable key
    /// encoding and the code entry it maps to, 0 when its code has none) unless the loaded one
    /// already covers it (on the owner thread).
    /// `stop` (a mid-run write: the main thread waits for the owner thread) is asked every 256
    /// slots; yes drops this record (false comes back) and a later write makes it again.
    pub fn record_manifest(&mut self, u: &Unit, root: u32, order: &[u32], present: usize, stop: &dyn Fn() -> bool) -> bool {
        let t0 = std::time::Instant::now();
        if !crate::preempt::is_stopped() && relax_codegen::warm::enabled() {
            for (id, f) in u.fns.iter().enumerate() {
                if !f.compiled && !f.history().inlined_into.is_empty() {
                    self.remember_outcome(u, id as u32, None, "cold inlined-only; no standalone code entry");
                }
            }
        }
        let _mem = crate::memstat::tag(crate::memstat::Tag::CacheStore);
        if self.mode == Mode::ReadOnly || order.is_empty() || (self.manifest_done && order.len() <= self.manifest_len && !self.image_rewrite && (self.stats.rewrite_why[0] + self.stats.rewrite_why[1]) as usize * 50 <= self.image_slots) {
            return true;
        }
        // at exit the slots' keys can no longer be made: after the mid-run manifest the exit
        // keeps the image (a rewrite there covers only the slots already keyed, and the
        // next start would prelink a few dozen slots instead of ~20,000). Slots whose entry
        // an O2/O3 build replaced since stay lazy at the next start (image.rs img_live).
        if self.manifest_done && crate::preempt::is_stopped() && !self.image_bad {
            return true;
        }
        let Some(key) = self.manifest_key(root) else { self.manifest_done = true; self.manifest_len = order.len(); return true };
        // what earlier spills of this run wrote: the image takes those entries too
        self.index_written();
        crate::run::yield_section("manifest keys");
        let mut list: Vec<(Vec<u8>, u128)> = Vec::with_capacity(order.len());
        let mut before_present = 0;
        let mut present_ids = Vec::new();
        for (n, &id) in order.iter().enumerate() {
            if n % 64 == 63 && stop() { return false; }
            if let Some(b) = self.slot_bytes(u, id) {
                let entry = match self.slot_key(u, id) {
                    Some(k) if self.has_index(&k) || self.pending_keys.contains(&k) => k,
                    _ => 0,
                };
                list.push((b, entry));
                if n < present {
                    before_present = list.len();
                    present_ids.push(id);
                }
            }
        }
        if self.verbose { eprintln!("makepad-run cache: manifest phases: {} slots keyed in {:.1} ms", order.len(), t0.elapsed().as_secs_f64() * 1e3); }
        if stop() { return false; }
        crate::run::yield_section("manifest compare");
        let old: HashSet<&[u8]> = self.manifest.iter().map(|b| b.as_slice()).collect();
        let new = list.iter().filter(|b| !old.contains(b.0.as_slice())).count();
        // the same start as before (within 2%), the loaded manifest still valid and its image
        // current (within 2%, and covering all but 2% of its cached slots): no new one
        let image_current = !self.image_rewrite
            && (self.image_slots == 0 || ((self.image_stale + (self.stats.rewrite_why[0] + self.stats.rewrite_why[1]) as usize) * 50 <= self.image_slots && self.image_slots * 50 >= self.manifest_present_cached * 49));
        if self.startup_manifest_valid() && new * 50 <= list.len() && image_current {
            self.manifest_done = true; self.manifest_len = order.len();
            return true;
        }
        if self.verbose {
            eprintln!("makepad-run cache: startup manifest rewritten: valid {} (missing {}), {} new of {}, image rewrite {} (replacements without a stored entry {}, tier-ups {}), image slots {} stale {} cached at present {}",
                self.startup_manifest_valid(), self.manifest_missing, new, list.len(), self.image_rewrite, self.stats.rewrite_why[0], self.stats.rewrite_why[1], self.image_slots, self.image_stale, self.manifest_present_cached);
        }
        crate::run::yield_section("manifest image");
        let img = if self.mode != Mode::Verify && !self.image_bad {
            match self.build_image(u, &present_ids, stop) { Err(()) => return false, Ok(img) => img }
        } else { None };
        // at exit no key can be made any more (slot_key): slots no lookup or record keyed this
        // run fall out of the image. A rewrite then (O2/O3 code replaced startup code since the
        // mid-run write) must not replace the loaded image with one covering fewer slots: the
        // next start would prelink a fraction of it (Calculator: 16,000 -> 2 slots)
        crate::run::yield_section("manifest tail");
        if stop() { return false; }
        let n_img = img.as_ref().map_or(0, |i| i.1 as usize);
        if crate::preempt::is_stopped() && self.image_slots > 0 && n_img * 50 < self.image_slots * 49 && !self.image_bad {
            if self.verbose {
                eprintln!("makepad-run cache: startup manifest kept: at exit the image would cover {} of the {} slots the loaded one has", n_img, self.image_slots);
            }
            self.manifest_done = true; self.manifest_len = order.len();
            return true;
        }
        // the entry first (the yield checks: nothing is queued before the last of them)
        crate::run::yield_section("manifest entry");
        let mut e = E::default();
        e.u32(ENTRY_VERSION);
        e.u32(list.len() as u32);
        e.u32(before_present as u32);
        for (i, (b, entry)) in list.iter().enumerate() {
            if i % 1024 == 1023 && stop() { return false; }
            e.bytes(b);
            e.u128(*entry);
        }
        if let Some((img, _)) = img {
            e.u32(IMAGE_MAGIC);
            for part in img.chunks(1 << 20) {
                e.0.extend_from_slice(part);
                if stop() { return false; }
            }
        }
        // the code entries this manifest supersedes: a slot of the loaded manifest whose key
        // changed (an edit reached what its code is made from) left its old entry behind under a
        // key nothing names any more (code stores are named by the crate's view, which an edit
        // keeps). Removed with this manifest's write, its bytes are dead space: the store
        // rewrite drops them once they outgrow a quarter of the live ones (store.rs append).
        crate::run::yield_section("manifest superseded");
        let mut gone: Vec<u128> = Vec::new();
        if self.manifest.len() == self.manifest_old_keys.len() {
            let old_keys: HashMap<&[u8], u128> = self.manifest.iter().zip(&self.manifest_old_keys)
                .filter(|(_, k)| **k != 0).map(|(b, k)| (b.as_slice(), *k)).collect();
            for (i, (b, k)) in list.iter().enumerate() {
                if i % 1024 == 1023 && stop() { return false; }
                if let Some(o) = old_keys.get(b.as_slice()).filter(|o| *k != 0 && **o != *k && !self.pending_keys.contains(o)) { gone.push(*o); }
            }
        }
        // (past the last yield check: this manifest is the run's)
        self.manifest_done = true; self.manifest_len = order.len();
        let mut removed = 0;
        for o in gone {
            let Some((p, _)) = self.index_at(o) else { continue };
            let Some(c) = self.code_path.iter().position(|cp| *cp == self.stores[p as usize].path) else { continue };
            self.push_pending(c as u32, NewEntry { key: o, kind: crate::store::KIND_REMOVED, payload: Vec::new() });
            removed += 1;
        }
        if self.verbose && removed != 0 { eprintln!("makepad-run cache: {} code entries superseded by this manifest: removed", removed); }
        self.push_pending(root, NewEntry { key, kind: KIND_MANIFEST, payload: e.0 });
        // the sources the keys above were made from (none after a live edit: this run's
        // fingerprints describe its start, not the edited files)
        let mut f = E::default();
        f.u128(if crate::reload::edited_crates().is_empty() { self.sources_fp() } else { 0 });
        self.push_pending(root, NewEntry { key: self.manifest_sources_key(key), kind: KIND_MANIFEST, payload: f.0 });
        self.image_rewrite = false;
        true
    }

    pub fn flush_front_graphs() -> Result<(), String> {
        flush_front_graphs()
    }

    pub fn flush(&mut self) -> Result<usize, String> {
        if self.verbose && self.mode == Mode::Verify {
            let s = &self.stats;
            eprintln!("makepad-run cache: verify: {} identical, {} different", s.verify_same, s.verify_diff);
        }
        if self.mode == Mode::ReadOnly {
            return Ok(0);
        }
        // data entries matter only to code that refers to them: alone (a function that then
        // could not be stored) they are not worth a write
        if !self.pending.iter().any(|e| e.1.kind != KIND_DATA) {
            return Ok(0);
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending_bytes = 0;
        self.pending_keys.clear();
        let n = pending.len();
        // the images the references among `pending` name, written by now
        let marks = front_run::run_marks();
        let late = crate::sip::late_inputs();
        pack::ensure_private_dir(&self.dir)?;
        let _io = SPILL_IO.write().unwrap_or_else(|e| e.into_inner());
        for ((c, front), mut entries) in by_store(pending) {
            let Some((path, stale)) = self.store_of(c, front) else { continue };
            let stale = std::mem::replace(stale, false);
            // the crate's ledger: the include!/env! inputs it read, as this run saw them
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let cname = name.rsplit_once('-').map_or("", |x| x.0).to_string();
            let mut e = E::default();
            let mine: Vec<_> = late.iter().filter(|x| x.0 == cname).collect();
            e.u32(mine.len() as u32);
            for (c, k, name, h) in mine {
                e.str(c);
                e.u8(*k);
                e.str(name);
                e.u128(*h);
            }
            entries.insert(0, NewEntry { key: LEDGER_KEY, kind: KIND_LEDGER, payload: e.0 }.into());
            append_store(&path, entries, stale, &marks)?;
        }
        if let Some(root) = self.dir.parent() {
            prune_root(root);
        }
        Ok(n)
    }
}

/// A slot identity's bytes (enc_fnkey) as a 128-bit hash: the manifest_keys index.
fn identity_hash(b: &[u8]) -> u128 {
    let mut h = Sip128::new();
    h.str("identity");
    h.write(b);
    h.finish128()
}

/// Pending entries by the store they go to: (crate, front store) -- front images to the crate's
/// front store, everything else to its code store.
fn by_store(pending: Vec<(u32, NewEntry)>) -> HashMap<(u32, bool), Vec<NewEntry>> {
    let mut by: HashMap<(u32, bool), Vec<NewEntry>> = HashMap::new();
    // Records arrive oldest first; store::append keeps the first entry for a duplicate key.
    // A later O3/retier must win over O1/O2 recorded in the same spill or exit flush.
    for (c, e) in pending.into_iter().rev() {
        let front = e.kind == KIND_FRONT || e.kind == KIND_FRONT_REF || e.kind == KIND_FRONT_CRATE;
        by.entry((c, front)).or_default().push(e);
    }
    by
}

impl Cache {
    /// Crate `c`'s front or code store file and its stale flag.
    fn store_of(&mut self, c: u32, front: bool) -> Option<(PathBuf, &mut bool)> {
        if front {
            Some((self.crate_path.get(c as usize)?.clone(), self.stale.get_mut(c as usize)?))
        } else {
            Some((self.code_path.get(c as usize)?.clone(), self.code_stale.get_mut(c as usize)?))
        }
    }
}

/// Entries taken out of a running cache (Cache::spill_take), written off the unit.
pub struct Spill {
    dir: PathBuf,
    files: Vec<(PathBuf, bool, Vec<NewEntry>)>,
    /// the images their references name, written by the time they were taken
    marks: front_run::RunMarks,
}

/// store::append for the crate and code stores: the run-store images any reference among
/// `entries` names (written by the time `marks` was taken) are committed first, so another
/// process never finds a reference to an image it cannot read.
fn append_store(path: &std::path::Path, entries: Vec<NewEntry>, stale: bool, marks: &front_run::RunMarks) -> Result<(), String> {
    if entries.iter().any(|e| e.kind == KIND_FRONT_REF) { front_run::commit_marks(marks)?; }
    crate::store::append(path, entries, stale).map_err(|e| format!("{}: {}", path.display(), e))
}

/// Spills/flushes exclude all writers. Cold writers share the gate only while holding
/// their store's lock, so independent stores can progress during serial collection.
pub static SPILL_IO: std::sync::RwLock<()> = std::sync::RwLock::new(());

// Weak entries coordinate concurrent compiler worlds without retaining old stores.
static PATHS: relax_session::sync::Mutex<Option<HashMap<PathBuf, std::sync::Weak<relax_session::sync::Mutex<()>>>>> = relax_session::sync::Mutex::new(None);

fn front_store_lock(path: &std::path::Path) -> std::sync::Arc<relax_session::sync::Mutex<()>> {
    let mut paths = PATHS.lock().unwrap_or_else(|e| e.into_inner());
    let paths = paths.get_or_insert_with(HashMap::new);
    paths.retain(|_, lock| lock.strong_count() != 0);
    if let Some(lock) = paths.get(path).and_then(std::sync::Weak::upgrade) { return lock; }
    let lock = std::sync::Arc::new(relax_session::sync::Mutex::new(()));
    paths.insert(path.to_owned(), std::sync::Arc::downgrade(&lock));
    lock
}

impl Spill {
    pub fn write(self) -> Result<usize, String> {
        let _g = SPILL_IO.write().unwrap_or_else(|e| e.into_inner());
        let late = crate::sip::late_inputs();
        self.write_locked(&_g, &late)
    }

    /// The caller holds SPILL_IO exclusively, or shared plus the cold store's lock.
    fn write_locked(self, _guard: &impl std::ops::Deref<Target = ()>, late: &[(String, u8, String, u128)]) -> Result<usize, String> {
        pack::ensure_private_dir(&self.dir)?;
        let mut n = 0;
        for (path, stale, mut entries) in self.files {
            let code: Vec<u128> = entries.iter().filter(|e| e.kind == KIND_CODE || e.kind == KIND_DATA).map(|e| e.key).collect();
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let cname = name.rsplit_once('-').map_or("", |x| x.0).to_string();
            let mut e = E::default();
            let mine: Vec<_> = late.iter().filter(|x| x.0 == cname).collect();
            e.u32(mine.len() as u32);
            for (c, k, name, h) in mine {
                e.str(c);
                e.u8(*k);
                e.str(name);
                e.u128(*h);
            }
            n += entries.len();
            entries.insert(0, NewEntry { key: LEDGER_KEY, kind: KIND_LEDGER, payload: e.0 }.into());
            append_store(&path, entries, stale, &self.marks)?;
            if !code.is_empty() {
                // process state: kept heap
                let mut w = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
                crate::zone_alloc::kept_with(|| w.push((self.dir.clone(), path.clone(), code.clone())));
            }
        }
        Ok(n)
    }
}

/// Code and data entries this run's spills wrote, by store file (cache dir, store, keys): the
/// owning cache indexes them (`Cache::index_written`) so the startup image is built from every code
/// entry the store has, not only the ones still pending. A cold run's first mid-run write spills
/// its code before the manifest window closes: its image covered 23-392 of ~45k slots, and the
/// first warm run rewrote the whole image (~435 ms on the owner) and started unprelinked.
static WRITTEN: relax_session::sync::Mutex<Vec<(PathBuf, PathBuf, Vec<u128>)>> = relax_session::sync::Mutex::new(Vec::new());

impl Cache {
    /// Indexes the code and data entries this run's spills wrote to this cache's stores
    /// (WRITTEN): the store file is opened again (mapped) and its new entries replace what the
    /// index had for their keys, as the file's own index does.
    fn index_written(&mut self) {
        let mine: Vec<(PathBuf, Vec<u128>)> = {
            let mut w = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
            // (what stays is another cache's, kept as it was: the list lives in the kept heap)
            let mut mine = Vec::new();
            let mut i = 0;
            while i < w.len() {
                if w[i].0 == self.dir {
                    let (_, path, keys) = &w[i];
                    mine.push((path.clone(), keys.clone()));
                    crate::zone_alloc::kept_with(|| drop(w.swap_remove(i)));
                } else {
                    i += 1;
                }
            }
            mine
        };
        for (path, keys) in mine {
            let Some(st) = Store::open(&path) else { continue };
            let keys: HashSet<u128> = keys.into_iter().collect();
            let si = self.stores.len() as u32;
            let mut n = 0;
            for (e, ix) in st.index.iter().enumerate() {
                let key = ix.key;
                if keys.contains(&key) {
                    self.index.insert(key, (si, e as u32));
                    n += 1;
                }
            }
            if n != 0 { self.stores.push(std::sync::Arc::new(st)); }
        }
    }
}

/// What a code record is: a first record (O1 / baseline / O3: recorded once, O3 replaces), an O2
/// build (replaces), a verified O1 rebuild (replaces only an entry below O1)
#[derive(Clone, Copy, PartialEq, Eq)]
enum RecKind { First, O2, O1Up }

/// A stored tier's order (the tier bytes are not in tier order): baseline < O1 < O2 < O3.
fn tier_rank(t: u8) -> u8 {
    match t { TIER_BASE => 0, TIER_O1 => 1, TIER_O2 => 2, TIER_O3 => 3, _ => 0 }
}

/// A code entry payload's tier byte.
fn payload_tier(p: &[u8]) -> Option<u8> {
    let mut d = D { b: p, p: 0 };
    if d.u32()? != ENTRY_VERSION { return None; }
    d.str()?;
    d.u8()
}

/// Whether a pack's recorded late macro inputs (include!/env! contents) still have the same
/// content: a pack from a run that read different bytes is not used.
fn ledger_current(p: &Store, present: &HashSet<String>, memo: &mut HashMap<(u8, String, String), u128>) -> bool {
    let payload = match p.get(LEDGER_KEY) {
        Some((KIND_LEDGER, b)) => b,
        _ => return false,
    };
    let mut d = D { b: payload, p: 0 };
    let n = match d.u32() {
        Some(n) => n,
        None => return false,
    };
    for _ in 0..n {
        let (c, k, name, h) = match (d.str(), d.u8(), d.str(), d.u128()) {
            (Some(c), Some(k), Some(n), Some(h)) => (c, k, n, h),
            _ => return false,
        };
        if !present.contains(c) {
            continue;
        }
        // each input is read once per open, however many packs name it
        let now = *memo.entry((k, c.to_string(), name.to_string())).or_insert_with(|| {
            if k == crate::sip::LATE_FILE {
                crate::sip::late_hash(crate::vfs::read(name).ok().as_deref())
            } else {
                crate::sip::late_hash(crate::crates::crate_env_named(c, name).as_deref().map(|s| s.as_bytes()))
            }
        });
        if now != h {
            return false;
        }
    }
    true
}

/// The definitions whose bodies a fn key's code is made from: an instance's definition, and a
/// closure's enclosing fns up to that instance (the closure's own text is in its key: file
/// version and position).
fn key_defs(u: &Unit, key: &FnKey) -> Vec<DefId> {
    let mut out = Vec::new();
    let mut k = key.clone();
    for _ in 0..32 {
        match k {
            FnKey::Inst(d, _) => { out.push(d); break; }
            FnKey::Closure(_, _, parent) | FnKey::Plain(parent) if (parent as usize) < u.fns.len() => k = u.fns[parent as usize].key.clone(),
            _ => break,
        }
    }
    out
}

fn foreign_def_of(u: &Unit, s: Sym) -> Option<DefId> {
    u.syms.foreign_def(s.id)
}

impl Cache {
    /// The canonical declaration of foreign item `d`: the first (smallest DefId; load order is
    /// fixed for a program) foreign item with its link name. A C symbol is often declared in
    /// several extern blocks (`__errno_location`), and which declaration a run met first
    /// depended on compile order -- the entry's encoding with it.
    fn canon_foreign(&mut self, u: &Unit, d: DefId) -> DefId {
        if self.foreign_canon.is_none() {
            let mut m: HashMap<String, DefId> = HashMap::new();
            for i in 0..u.prog.defs.len() {
                let di = DefId(i as u32);
                if matches!(u.prog.def(di).kind, DefKind::ForeignFn | DefKind::ForeignStatic) {
                    m.entry(u.link_name(di)).or_insert(di);
                }
            }
            self.foreign_canon = Some(m);
        }
        let name = u.link_name(d);
        self.foreign_canon.as_ref().and_then(|m| m.get(&name).copied()).unwrap_or(d)
    }
}

/// Cache keys are SipHash-128 outputs (uniform): their low 64 bits are the map hash.
#[derive(Default, Clone, Copy)]
pub struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, b: &[u8]) {
        for (i, x) in b.iter().take(8).enumerate() {
            self.0 ^= (*x as u64) << (8 * i);
        }
    }
    fn write_u128(&mut self, v: u128) {
        self.0 ^= v as u64;
    }
}

pub type KeyMap<V> = HashMap<u128, V, std::hash::BuildHasherDefault<KeyHasher>>;

pub fn app_store_files(dir: &std::path::Path, krate: &str) -> Vec<PathBuf> {
    let prefix = format!("{krate}-");
    let mut bases: Vec<PathBuf> = crate::store::stores_with_prefix(dir, &prefix).into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n.len() == prefix.len() + 20)).collect();
    for g in crate::store::stores_with_prefix(dir, "front_graph-") {
        let Some(st) = Store::open(&g) else { continue };
        for i in 0..st.index.len() {
            let Some(e) = st.index.get(i).copied() else { continue };
            let (key, kind) = (e.key, e.kind);
            if kind != KIND_FRONT_GRAPH { continue; }
            let Some((_, bytes)) = st.get_at(i, key) else { continue };
            let Some(graph) = crate::crate_image::decode::<crate::crate_image::GraphImage>(bytes) else { continue };
            if graph.crates.iter().any(|((name, _), _)| name == krate) {
                bases.push(g.clone());
                bases.push(collected::path(dir, graph.identity));
            }
        }
    }
    bases.sort();
    bases.dedup();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name();
        let Some(n) = n.to_str() else { continue };
        if bases.contains(&dir.join(crate::store::split_gen(n).0)) {
            files.push(dir.join(n));
        }
    }
    files.sort();
    files
}

/// The cap's floor (MB) when MAKEPAD_RUN_CACHE_MAX is unset.
const CAP_FLOOR_MB: u64 = 4096;
/// A store stamped this recently is taken to be in use by another run (every run stamps the
/// stores it opens: store::mark_live).
const IN_USE_BY_OTHERS: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Keeps the cache: first every store's older generations go (store.rs: a rewrite leaves the
/// generation it replaced for readers that still have it open), except one this process
/// itself opened; a delete that fails (open elsewhere, on Windows) is retried by the next
/// start. Then other Relax builds' stores go, unless a process of that build is running right
/// now (store::retire_build: every running process holds its build's live lock): store names
/// hash the codegen identity (codegen_id.rs), so no other build ever reads them. Then, while `<root>/cache` holds more
/// than the cap (MAKEPAD_RUN_CACHE_MAX megabytes, else twice what this run uses (its live
/// stores), at least CAP_FLOOR_MB), whole store files go, least recently used first (a run
/// stamps the files it uses; other crate versions' files are never stamped again) -- never one
/// this process opened or wrote, nor one another run stamped in the last IN_USE_BY_OTHERS.
/// Only checked store files are ever deleted. Returns (bytes before, after).
pub fn prune_root(root: &std::path::Path) -> (u64, u64) {
    let cap = std::env::var("MAKEPAD_RUN_CACHE_MAX").ok().and_then(|v| v.parse::<u64>().ok()).map(|mb| mb << 20);
    prune(root, &crate::store::live(), &crate::store::open_files(), &crate::store::programs(), cap)
}

/// At exit: every store's replaced generations go, including the ones this process itself
/// read (it is ending; on Windows a mapped file cannot be deleted and stays for the next
/// start's prune). Without this the generation a run's write replaced would stay on disk until
/// the next start (a whole copy of each rewritten store). `dir`: the cache folder.
pub fn retire_replaced_generations(dir: &std::path::Path) {
    let names: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().into_string().ok()).filter(|n| crate::store::name_ours(n)).collect();
    let mut newest: HashMap<String, u32> = HashMap::new();
    for n in &names {
        let (base, g) = crate::store::split_gen(n);
        let e = newest.entry(base.to_string()).or_insert(0);
        *e = (*e).max(g);
    }
    for n in &names {
        let (base, g) = crate::store::split_gen(n);
        if g < newest[base] {
            crate::store::delete_checked(&dir.join(n));
        }
    }
}

/// retire_replaced_generations in the folders of the stores this process opened or wrote.
pub fn retire_replaced_in_use() {
    let dirs: HashSet<PathBuf> = crate::store::live().iter().filter_map(|p| p.parent().map(|d| d.to_path_buf())).collect();
    for d in dirs { retire_replaced_generations(&d); }
}

/// prune_root with this run's stores `live` (generation 0 paths), the generation files it opened
/// `open`, and the cap in bytes (None: the default).
fn prune(root: &std::path::Path, live: &HashSet<PathBuf>, open: &HashSet<PathBuf>, programs: &[(u64, std::collections::BTreeMap<String, String>)], cap: Option<u64>) -> (u64, u64) {
    let mut before = 0u64;
    let dir = root.join("cache");
    let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                let p = dir.join(&n);
                let m = std::fs::symlink_metadata(&p).ok()?;
                if !crate::store::name_ours(&n) || !m.file_type().is_file() {
                    return None;
                }
                Some((m.modified().unwrap_or(std::time::UNIX_EPOCH), p, m.len()))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut newest: HashMap<PathBuf, u32> = HashMap::new();
    for f in &files {
        let g = crate::store::split_gen(&f.1.file_name().unwrap_or_default().to_string_lossy()).1;
        let e = newest.entry(crate::store::base_path(&f.1)).or_insert(0);
        *e = (*e).max(g);
    }
    before += files.iter().map(|f| f.2).sum::<u64>();
    files.retain(|f| {
        let base = crate::store::base_path(&f.1);
        let g = crate::store::split_gen(&f.1.file_name().unwrap_or_default().to_string_lossy()).1;
        let stale = g < newest[&base] && !open.contains(&f.1);
        !(stale && crate::store::delete_checked(&f.1))
    });
    // other builds' stores, by build (a store being written right now, or unreadable, has no
    // build here and is left to the cap)
    let me = crate::store::this_build();
    let builds: HashMap<PathBuf, Option<u64>> = files.iter().map(|f| (f.1.clone(), crate::store::build_of(&f.1))).collect();
    let mut other: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for f in &files {
        if let Some(b) = builds[&f.1].filter(|b| *b != me) {
            other.entry(b).or_default().push(f.1.clone());
        }
    }
    let sets = dir.join("sets");
    // ended builds whose stores are gone leave only their lock file
    for b in crate::store::lock_builds(&dir) {
        if b != me {
            other.entry(b).or_default();
        }
    }
    let mut retired: HashSet<PathBuf> = HashSet::new();
    let mut running: HashSet<PathBuf> = HashSet::new();
    for (b, paths) in &other {
        let idle = crate::store::retire_build(&dir, *b, || {
            for p in paths {
                if crate::store::delete_checked(p) {
                    retired.insert(p.clone());
                }
            }
            for n in set_files(&sets, *b) {
                let _ = std::fs::remove_file(sets.join(n));
            }
            let _ = std::fs::remove_file(retiring_path(&sets, *b));
        });
        if !idle {
            running.extend(paths.iter().cloned());
        }
    }
    files.retain(|f| !retired.contains(&f.1));
    let now = std::time::SystemTime::now();
    let recent = |t: &std::time::SystemTime| now.duration_since(*t).map_or(true, |age| age < IN_USE_BY_OTHERS);
    // this build's replaced crate stores (store sets, write_sets): one a program's set named and
    // its current set no longer names goes once no set or demand part names it, this run does
    // not use it and no run stamped it recently (until then it stays on the retiring list)
    let name = |p: &PathBuf| crate::store::base_path(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let keep = |f: &(std::time::SystemTime, PathBuf, u64)| builds.get(&f.1).copied().flatten() != Some(me)
        || live.contains(&crate::store::base_path(&f.1)) || open.contains(&f.1) || recent(&f.0);
    let (named, retiring) = write_sets(&sets, me, programs);
    let mut part_named: HashSet<String> = HashSet::new();
    // the front stores demand parts take images from (each graph store lists them); a graph
    // store that cannot be read right now (being written) may name any of them
    let parts_known = files.iter().filter(|f| family(&name(&f.1)) == "front_graph").all(|f| match crate::store::peek(&f.1, KIND_DEMAND_STORES) {
        Some(lists) => {
            part_named.extend(lists.iter().flat_map(|b| String::from_utf8_lossy(b).lines().map(str::to_string).collect::<Vec<_>>()));
            true
        }
        None => false,
    });
    if parts_known {
        files.retain(|f| {
            let n = name(&f.1);
            !(retiring.contains(&n) && !named.contains(&n) && !part_named.contains(&n) && !keep(f) && crate::store::delete_checked(&f.1))
        });
    }
    // what stays retiring: still on disk (in use or stamped recently now) and named by no set
    if !retiring.is_empty() || retiring_path(&sets, me).exists() {
        let left: std::collections::BTreeSet<String> = files.iter().map(|f| name(&f.1)).filter(|n| retiring.contains(n) && !named.contains(n)).collect();
        let text: String = left.iter().map(|n| format!("{n}\n")).collect();
        let _ = pack::write_atomic_with(&sets, &format!("{me:016x}.retiring"), |f| std::io::Write::write_all(f, text.as_bytes()));
    }
    // run stores (file images) no store refers to: once per process (the scan reads every store's
    // references), at its first prune
    static RUNS_CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if files.iter().any(|f| family(&name(&f.1)) == RUN_STORE_FAMILY && !keep(f)) && !RUNS_CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        // a store that cannot be read right now (a writer has it) may refer to any of them
        let referenced: Option<HashSet<String>> = files.iter().filter(|f| family(&name(&f.1)) != RUN_STORE_FAMILY)
            .map(|f| crate::store::peek(&f.1, KIND_FRONT_REF)).collect::<Option<Vec<_>>>()
            .map(|v| v.iter().flatten().filter_map(|b| front_run::decode_ref(b).map(|(n, _)| n.to_string())).collect());
        if let Some(referenced) = referenced { files.retain(|f| {
            let n = name(&f.1);
            !(family(&n) == RUN_STORE_FAMILY && !referenced.contains(&n) && !keep(f) && crate::store::delete_checked(&f.1))
        }); }
    }
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    let live_bytes: u64 = files.iter().filter(|f| live.contains(&crate::store::base_path(&f.1))).map(|f| f.2).sum();
    let cap = cap.unwrap_or((CAP_FLOOR_MB << 20).max(2 * live_bytes));
    files.sort();
    let mut evicted: HashSet<&PathBuf> = HashSet::new();
    for (t, p, len) in &files {
        if total <= cap {
            break;
        }
        let in_use = live.contains(&crate::store::base_path(p)) || recent(t);
        if !in_use && crate::store::delete_checked(p) {
            total -= len;
            evicted.insert(p);
        }
    }
    static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if total > cap && !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        // what stayed over the cap, and why: this run's stores, then other running builds'
        // stores, then stores another run of this build stamped recently
        let (mut other_running, mut other_runs) = (0u64, 0u64);
        for (t, p, len) in files.iter().filter(|f| !evicted.contains(&f.1) && !live.contains(&crate::store::base_path(&f.1))) {
            if running.contains(p) {
                other_running += len;
            } else if recent(t) {
                other_runs += len;
            }
        }
        eprintln!(
            "makepad-run cache: {} MB stays over the cap ({} MB): this run's stores {} MB, running other builds' {} MB, stores other runs used in the last {} min {} MB",
            total >> 20, cap >> 20, live_bytes >> 20, other_running >> 20, IN_USE_BY_OTHERS.as_secs() / 60, other_runs >> 20
        );
    }
    (before, total)
}

/// The run stores' family (front_run.rs): file images, kept while a store refers to them.
const RUN_STORE_FAMILY: &str = "front_run";

/// A store name's family: the crate (or `front_graph`, `collected_graph`, `front_run`, ...)
/// before its 16 hex.
fn family(name: &str) -> &str {
    name.rsplit_once('-').map_or(name, |x| x.0)
}

/// The set files of build `build` in `sets` (`<build hex>-<program hex>`).
fn set_files(sets: &std::path::Path, build: u64) -> Vec<String> {
    let prefix = format!("{build:016x}-");
    std::fs::read_dir(sets).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.len() == 33 && n.starts_with(&prefix) && n.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
        .collect()
}

/// Store sets: each program this process runs (`programs`, store::note_program) names the
/// stores it uses, one per role (a crate's front store, its code store, the program's graph
/// stores), in `<cache>/sets/<build>-<program>` (`role name` lines). A crate store the previous
/// run's set named and the program's set no longer names was replaced (an edit changed the
/// crate's fingerprint, or the crate left the program): it joins `<cache>/sets/<build>.retiring`
/// until it can go (prune; a retained demand part still naming an old front store keeps it,
/// KIND_DEMAND_STORES). Returns every store name the build's sets hold now, and the retiring list.
fn write_sets(sets: &std::path::Path, build: u64, programs: &[(u64, std::collections::BTreeMap<String, String>)]) -> (HashSet<String>, HashSet<String>) {
    let mut retiring: HashSet<String> = std::fs::read_to_string(retiring_path(sets, build)).unwrap_or_default()
        .lines().filter(|n| crate::store::name_ours(n)).map(str::to_string).collect();
    if !programs.is_empty() && pack::ensure_private_dir(sets).is_ok() {
        for (prog, roles) in programs {
            let file = format!("{build:016x}-{prog:016x}");
            let prev = std::fs::read_to_string(sets.join(&file)).unwrap_or_default();
            let now: HashSet<&String> = roles.values().collect();
            retiring.extend(prev.lines().filter_map(|l| l.rsplit_once(' ')).filter(|(r, _)| r.starts_with("code ") || r.starts_with("front "))
                .map(|(_, n)| n.to_string()).filter(|n| !now.contains(n) && crate::store::name_ours(n)));
            let text: String = roles.iter().map(|(r, n)| format!("{r} {n}\n")).collect();
            let _ = pack::write_atomic_with(sets, &file, |f| std::io::Write::write_all(f, text.as_bytes()));
        }
    }
    let named = set_files(sets, build).iter()
        .flat_map(|n| std::fs::read_to_string(sets.join(n)).unwrap_or_default().lines().filter_map(|l| l.rsplit_once(' ').map(|x| x.1.to_string())).collect::<Vec<_>>())
        .collect();
    (named, retiring)
}

/// The build's list of replaced code stores not deleted yet (write_sets).
fn retiring_path(sets: &std::path::Path, build: u64) -> PathBuf {
    sets.join(format!("{build:016x}.retiring"))
}

/// Each crate's longest dependency chain (core 0).
fn crate_depths(prog: &crate::program::Program) -> Vec<u32> {
    let n = prog.crates.len();
    let mut d: Vec<Option<u32>> = vec![None; n];
    fn go(prog: &crate::program::Program, c: usize, d: &mut Vec<Option<u32>>, guard: u32) -> u32 {
        if let Some(x) = d[c] {
            return x;
        }
        if guard > 256 {
            return 0;
        }
        d[c] = Some(0);
        let v = prog.crates[c].deps.iter().map(|(_, dc)| go(prog, *dc as usize, d, guard + 1) + 1).max().unwrap_or(0);
        d[c] = Some(v);
        v
    }
    (0..n).map(|c| go(prog, c, &mut d, 0)).collect()
}

fn prim_code(p: crate::program::Prim) -> u8 {
    use crate::program::Prim::*;
    match p {
        I8 => 1,
        I16 => 2,
        I32 => 3,
        I64 => 4,
        I128 => 5,
        Isize => 6,
        U8 => 7,
        U16 => 8,
        U32 => 9,
        U64 => 10,
        U128 => 11,
        Usize => 12,
        F32 => 13,
        F64 => 14,
        _ => 0,
    }
}

fn prim_of(c: u8) -> Option<crate::program::Prim> {
    use crate::program::Prim::*;
    Some(match c {
        1 => I8,
        2 => I16,
        3 => I32,
        4 => I64,
        5 => I128,
        6 => Isize,
        7 => U8,
        8 => U16,
        9 => U32,
        10 => U64,
        11 => U128,
        12 => Usize,
        13 => F32,
        14 => F64,
        _ => return None,
    })
}

/// A definition's key: its parent's key (the crate's identity at the root), name, kind,
/// defining file and, for an item its path does not name (block-scoped, `const _`), its offset
/// there -- the identity stable::def_ident gives TypeId (memoized in `memo`; parents are in
/// the same crate).
fn def_key_in(prog: &crate::program::Program, tcx: &crate::tcx::Tcx, crate_ident: &[u128], memo: &mut HashMap<DefId, u128>, files: &mut HashMap<u32, u128>, d: DefId) -> u128 {
    if let Some(&k) = memo.get(&d) {
        return k;
    }
    let def = prog.def(d);
    let parent = if def.parent == NO_DEF { crate_ident.get(def.krate as usize).copied().unwrap_or(u128::MAX ^ def.krate as u128) } else { def_key_in(prog, tcx, crate_ident, memo, files, def.parent) };
    let fk = *files.entry(def.file).or_insert_with(|| hash_bytes(prog.file_paths.get(def.file as usize).map_or("", |p| p.as_str()).as_bytes()));
    let mut h = Sip128::new();
    h.u128(parent);
    h.str(prog.syms.str(def.name));
    h.u32(def_kind_code(&def.kind));
    h.u128(fk);
    // the item's offset only when its path does not name it -- the identity TypeId uses
    // (stable::def_ident): a key and the TypeId folded into its code never disagree
    if crate::stable::ident_has_offset(prog, d) {
        h.u32(prog.files.get(def.file as usize).and_then(|f| f.ast.items.get(def.item.0 as usize)).map_or(u32::MAX, |it| it.lo));
    }
    // not def.sub: module / impl indices are global to the program (they differ per app).
    // Impls (derived ones share their type's item) by what they implement for what.
    if def.kind == DefKind::Impl {
        if let Some(t) = tcx.impl_self.get(&d) {
            h.u128(crate::stable::type_hash(&prog, &tcx, *t));
        }
        if let Some(tr) = tcx.impl_trait.get(&d) {
            let mut th = Sip128::new();
            crate::stable::def_ident(&prog, *tr, &mut th);
            h.u128(th.finish128());
        }
        if let Some(args) = tcx.impl_trait_args.get(&d) {
            for t in args {
                h.u128(crate::stable::type_hash(&prog, &tcx, *t));
            }
        }
    }
    let k = h.finish128();
    memo.insert(d, k);
    k
}

fn def_kind_code(k: &DefKind) -> u32 {
    match k {
        DefKind::Mod => 1,
        DefKind::Fn => 2,
        DefKind::Struct => 3,
        DefKind::Union => 4,
        DefKind::Enum => 5,
        DefKind::Variant => 6,
        DefKind::Const => 7,
        DefKind::Static => 8,
        DefKind::Trait => 9,
        DefKind::Impl => 10,
        DefKind::TypeAlias => 11,
        DefKind::AssocFn => 12,
        DefKind::AssocConst => 13,
        DefKind::AssocTy => 14,
        DefKind::ForeignFn => 15,
        DefKind::ForeignStatic => 16,
        DefKind::ForeignTy => 22,
        DefKind::MacroRules => 17,
        DefKind::AnonConst => 18,
        DefKind::OpaqueTy => 19,
        DefKind::Closure => 20,
        DefKind::Field => 21,
        DefKind::Prim(p) => 100 + prim_code(*p) as u32,
    }
}

/// MAKEPAD_RUN_CACHE_TRACE=<file>: one line per lookup (ms since start, hit/miss, thread, crate, slot
/// name, stable name), kept in memory and written at exit (no I/O on the compile path).
static TRACE: relax_session::sync::Mutex<Option<String>> = relax_session::sync::Mutex::new(None);

pub(crate) fn trace_on() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("MAKEPAD_RUN_CACHE_TRACE").is_some())
}

pub(crate) fn trace_lookup(u: &Unit, id: u32, what: &str) {
    trace_keyed(u, id, what, None)
}

/// trace_lookup with the code key given (a record runs with the cache taken out of the unit).
pub(crate) fn trace_keyed(u: &Unit, id: u32, what: &str, code_key: Option<u128>) {
    if !trace_on() {
        return;
    }
    let krate = match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => u.prog.syms.str(u.prog.crates[u.prog.def(*d).krate as usize].name).to_string(),
        FnKey::Closure(f, _, _) => u.prog.file_crate.get(*f as usize).map_or("-".to_string(), |c| u.prog.syms.str(u.prog.crates[*c as usize].name).to_string()),
        FnKey::Shim(_) | FnKey::Glue(..) | FnKey::Sandbox(_) | FnKey::Plain(_) => "-".to_string(),
    };
    let on_main = crate::owner::for_main();
    // the code key and the identity of the fn (its key bytes without interfaces and bodies): the same
    // identity under another key in two runs is a function the cache failed to share
    let (key, ident) = u.cache.as_ref().map_or((0, 0), |c| (c.slot_keys.get(&id).copied().flatten().unwrap_or(0), c.fnkey_bytes.get(&id).and_then(|b| b.as_ref()).map_or(0, |b| identity_hash(b))));
    let key = code_key.unwrap_or(key);
    let line = format!("{:.1}\t{}\t{}\t{}\t{}\t{}\t{:032x}\t{:032x}\n", crate::jit::wall_ns() as f64 / 1e6, what, if on_main { "main" } else { "worker" }, krate, u.fns[id as usize].name, crate::syms::stable_fn_name(u, id), key, ident);
    crate::zone_alloc::kept_with(|| TRACE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(String::new).push_str(&line));
}

/// Writes MAKEPAD_RUN_CACHE_TRACE (at exit).
pub fn write_trace() {
    if let Ok(p) = std::env::var("MAKEPAD_RUN_CACHE_TRACE") {
        // ({pid}: one file per process -- a program's build scripts run under the same env)
        let p = p.replace("{pid}", &std::process::id().to_string());
        // (the whole trace each time: the quit write and the exit both write it)
        if let Some(t) = TRACE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = std::fs::write(p, t);
        }
    }
}

/// The address of a provided (host-built) crate's entry in this process (SymKind::Provided).
fn provided_value(u: &Unit, name: &str) -> Option<u64> {
    if let Some(key) = name.strip_prefix("shim:") {
        for (_, pc) in &u.provided {
            if let Some(sh) = pc.shims.get(key) {
                return Some(sh.f as usize as u64);
            }
        }
        return None;
    }
    if let Some(c) = name.strip_prefix("last_panic:") {
        return u.provided.iter().find(|(_, pc)| pc.name == c).map(|(_, pc)| pc.last_panic as usize as u64);
    }
    if let Some(path) = name.strip_prefix("drop:") {
        return u.provided_types.iter().find(|(d, _)| u.prog.def_path(**d) == path).map(|(_, a)| *a).filter(|a| *a != 0);
    }
    None
}

impl Cache {
    /// Verify mode: the entry this run would record for slot `id` (fresh code + relocs) against
    /// the cached entry: same declared nounwind inputs, pool-zeroed code and encoded
    /// relocations. Prints the first difference, including changed conservative answers.
    pub fn verify_entry(&mut self, u: &Unit, id: u32, fresh: &[u8], relocs: &[RelocAt]) -> bool {
        let key = match self.slot_key(u, id) {
            Some(k) => k,
            None => return true,
        };
        let cached = match self.get(key) {
            Some((KIND_CODE, p)) => p.to_vec(),
            _ => return true,
        };
        let mut d = D { b: &cached, p: 0 };
        if d.u32() != Some(ENTRY_VERSION) || d.str().is_none() { return false; }
        let Some((start, end)) = skip_build_inputs(&mut d) else { return false };
        let facts = u.fns[id as usize].rir.as_ref().map_or(&[][..], |r| r.nounwind_inputs.as_slice());
        let Some(encoded) = self.nounwind_bytes(u, facts) else { return false };
        if cached[start..end] != encoded {
            eprintln!("makepad-run cache: VERIFY nounwind inputs differ: {}", crate::syms::stable_fn_name(u, id));
            return false;
        }
        let Some(ccode) = d.bytes() else { return false };
        let ccode = ccode.to_vec();
        let fz = fresh.to_vec();
        let name = crate::syms::stable_fn_name(u, id);
        if fz != ccode {
            let mut diffs = Vec::new();
            for k in (0..fz.len().min(ccode.len())).step_by(4) {
                if fz[k..k + 4] != ccode[k..k + 4] && diffs.len() < 4 {
                    diffs.push(format!("{}: {:08x}/{:08x}", k, u32::from_le_bytes(ccode[k..k + 4].try_into().unwrap()), u32::from_le_bytes(fz[k..k + 4].try_into().unwrap())));
                }
            }
            eprintln!("makepad-run cache: VERIFY code differs: {} ({} vs {} bytes) cached/fresh {:?}", name, ccode.len(), fz.len(), diffs);
            // the fresh relocations whose encoding the cached entry does not hold (what the two
            // builds' constant pools disagree on)
            if dump_filter().map_or(false, |w| name.contains(w)) {
                let rest = &cached[d.p..];
                let n_cached = d.clone_u32_at_rest();
                let mut missing = Vec::new();
                let mut rw = Vec::new();
                for rl in relocs {
                    let mut e = E::default();
                    let ok = match &rl.target {
                        Reloc::Sym(s) => self.enc_sym(u, *s, &mut e, &mut rw).is_some(),
                        Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => self.slot_bytes(u, *x).map(|b| e.bytes(&b)).is_some(),
                        _ => false,
                    };
                    if ok && !rest.windows(e.0.len().max(1)).any(|w| w == &e.0[..]) {
                        missing.push(match &rl.target {
                            Reloc::Sym(s) => format!("{} {}", rl.at, crate::syms::describe(u, *s)),
                            Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => format!("{} slot {}", rl.at, crate::syms::stable_fn_name(u, *x)),
                            _ => String::new(),
                        });
                    }
                }
                eprintln!("makepad-run cache: VERIFY {}: cached {} relocations, fresh {}; fresh ones the cached entry lacks: {:?}", name, n_cached, relocs.len(), missing);
                for rl in relocs {
                    let t = match &rl.target {
                        Reloc::Sym(s) => format!("{} +{}", crate::syms::describe(u, *s), s.off),
                        Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => format!("slot {}", crate::syms::stable_fn_name(u, *x)),
                        r => format!("{:?}", r),
                    };
                    eprintln!("makepad-run cache: RELOCS fresh {} {} {}", name, rl.at, t);
                }
            }
            return false;
        }
        let n = d.u32().unwrap_or(0) as usize;
        let start = d.p;
        let _ = start;
        let mut rw = Vec::new();
        for rl in relocs.iter().take(n) {
            let (o, r) = (&rl.at, &rl.target);
            let co = d.u32().unwrap_or(u32::MAX);
            let ck = d.u8().and_then(kind_of);
            let mut e = E::default();
            let ok = match r {
                Reloc::Sym(s) => self.enc_sym(u, *s, &mut e, &mut rw).is_some(),
                Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => {
                    e.u8(match r { Reloc::Slot(_) => 20, Reloc::Call(_) => 25, _ => 21 });
                    match self.slot_bytes(u, *x) {
                        Some(b) => {
                            e.bytes(&b);
                            true
                        }
                        None => false,
                    }
                }
                Reloc::TlsKey => { e.u8(22); true }
                Reloc::TlsSlow => { e.u8(23); true }
                Reloc::Resume => { e.u8(24); true }
                Reloc::Value(_) => false,
            };
            let cbytes = d.b.get(d.p..d.p + e.0.len()).map(|x| x.to_vec());
            if co != *o || ck != Some(rl.kind) || !ok || cbytes.as_deref() != Some(&e.0[..]) {
                eprintln!("makepad-run cache: VERIFY RELOC differs: {} at {} ({:?} -> {})", name, o, r, match r { Reloc::Sym(s) => crate::syms::describe(u, *s), Reloc::Slot(x) | Reloc::Thunk(x) | Reloc::Call(x) => crate::syms::stable_fn_name(u, *x), _ => String::new() });
                return false;
            }
            d.p += e.0.len();
        }
        true
    }
}

/// MAKEPAD_RUN_CACHE_DUMP=<name substring>: link details of matching entries (read once).
fn dump_filter() -> Option<&'static str> {
    static F: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    relax_session::phase::env_once(&F, "MAKEPAD_RUN_CACHE_DUMP")
}

fn kind_code(k: RelocKind) -> u8 {
    match k {
        RelocKind::Pool64 => 0,
        RelocKind::X64Rel32 => 1,
        RelocKind::A64Adrp => 2,
        RelocKind::A64AddLo12 => 3,
        RelocKind::A64LdrLo12X => 4,
        RelocKind::Imm32 => 5,
        RelocKind::A64MovzMovk => 6,
        RelocKind::A64CoverageBit { .. } => unreachable!("coverage disables the code cache"),
        RelocKind::X64Branch32 => 7,
        RelocKind::A64Branch26 => 8,
    }
}

fn kind_of(c: u8) -> Option<RelocKind> {
    Some(match c {
        0 => RelocKind::Pool64,
        1 => RelocKind::X64Rel32,
        2 => RelocKind::A64Adrp,
        3 => RelocKind::A64AddLo12,
        4 => RelocKind::A64LdrLo12X,
        5 => RelocKind::Imm32,
        6 => RelocKind::A64MovzMovk,
        7 => RelocKind::X64Branch32,
        8 => RelocKind::A64Branch26,
        _ => return None,
    })
}

/// Skips a code entry's capture section (Cache::capture_section).
/// A closure's identity in its parse (closure keys and capture sections carry it): the text of
/// its expression (an async fn body: its block) and where that text was authored. A closure's
/// expression id is a position in one parse's arenas, so a key holds what that position must
/// still be. None: this run has not parsed the expression.
fn closure_text_fp(u: &Unit, file: u32, expr: u32) -> Option<u64> {
    let pf = u.prog.files.get(file as usize)?;
    let body = expr & crate::tcx::ASYNC_FN_BODY != 0;
    let (lo, hi) = if body {
        let b = crate::ast::BlockId(expr & !crate::tcx::ASYNC_FN_BODY);
        if !pf.ast.has_block(b) {
            return None;
        }
        let b = pf.ast.block(b);
        (b.lo, b.hi)
    } else {
        let x = crate::ast::ExprId(expr);
        if !pf.ast.has_expr(x) {
            return None;
        }
        let x = pf.ast.expr(x);
        (x.lo, x.hi)
    };
    let at = pf.source_span(lo);
    let mut h = Sip128::new();
    h.str("closure");
    h.u8(body as u8);
    h.write(pf.bytes(lo, hi));
    h.u32(at.lo);
    h.u32(at.hi);
    Some(h.finish128() as u64)
}

/// Build check of closure slot `id` (jit, before lowering it from its parent's body): false
/// when a cache entry this run linked named its expression as a closure with another identity.
pub(crate) fn closure_identity_ok(u: &Unit, file: u32, expr: u32) -> bool {
    let Some(c) = u.cache.as_ref() else { return true };
    let want = c.closure_expected_fp(file, expr);
    match (want, closure_text_fp(u, file, expr)) {
        (Some(w), Some(have)) => w == have,
        _ => true,
    }
}


/// Reads the nounwind inputs and the dependency certificate; returns the nounwind section (for
/// verification).
/// A file version's identity in an entry: a hash of its path, source and owning crate (one
/// 16-byte id where the path string and two hashes were stored per file and entry).
fn file_ident(path: &str, source: u128, owner: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("file-ident");
    h.str(path);
    h.u128(source);
    h.u128(owner);
    h.finish128()
}

fn skip_build_inputs(d: &mut D) -> Option<(usize, usize)> {
    // the O3 byte and the inlined slots
    d.u8()?;
    let n = d.u32()?;
    for _ in 0..n {
        d.bytes()?;
    }
    skip_nounwind_and_cert(d)
}

/// The nounwind inputs and the dependency certificate; returns the nounwind subsection.
fn skip_nounwind_and_cert(d: &mut D) -> Option<(usize, usize)> {
    let start = d.p;
    let n = d.u32()?;
    for _ in 0..n { d.bytes()?; d.u8()?; }
    let end = d.p;
    // the dependency certificate
    let nc = d.u32()?;
    for _ in 0..nc { d.u128()?; }
    d.u128()?;
    let n = d.u32()?;
    for _ in 0..n { d.bytes()?; d.u128()?; }
    Some((start, end))
}

// Shared with pre-parse discovery: keep the filename's build/target/fingerprint formula in
// one place. Front metadata has its own versioned key in addition to the codegen identity.
fn front_store_name(options: &crate::options::Options, name: &str, fp: u128) -> String {
    front_store_name_with(crate::crate_image::parse_options(options), name, fp)
}
/// front_store_name with the options' parse fingerprint computed once for many crates
fn front_store_name_with(parse_options: u128, name: &str, fp: u128) -> String {
    let mut h = Sip128::new();
    h.str("store");
    h.u128(crate::codegen_id::id());
    h.str(&crate::codegen_id::target_tag());
    h.u128(fp);
    // parser policy changes the trees (and their ids) under the same sources
    h.u8(crate::parser::lazy_auto() as u8);
    h.u128(parse_options);
    format!("{}-{:016x}.rcs", name, h.finish128() as u64)
}
/// A crate's code store: the front store's formula over its interface fingerprint (another
/// hash domain, so never the front store's name).
fn code_store_name(options: &crate::options::Options, name: &str, iface: u128) -> String {
    let mut h = Sip128::new();
    h.str("code-store");
    h.u128(crate::codegen_id::id());
    h.str(&crate::codegen_id::target_tag());
    h.u128(iface);
    h.u8(crate::parser::lazy_auto() as u8);
    h.u128(crate::crate_image::parse_options(options));
    format!("{}-{:016x}.rcs", name, h.finish128() as u64)
}
/// A crate image's key: name and root (a package's lib and bin crates share the name)
fn front_crate_key(name: &str, root: &str) -> u128 {
    let mut h = Sip128::new();
    h.str("front-crate");
    h.u32(crate::crate_image::VERSION);
    h.str(name);
    h.str(root);
    h.finish128()
}

fn front_graph_key(identity: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("front-graph");
    h.u128(crate::codegen_id::id());
    h.str(&crate::codegen_id::target_tag());
    h.u128(identity);
    h.finish128()
}
fn front_order_key(set: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("front-order");
    h.u128(crate::codegen_id::id());
    h.str(std::env::consts::ARCH); h.str(std::env::consts::OS);
    h.u128(set);
    h.finish128()
}
fn front_order_path(dir: &std::path::Path, set: u128) -> PathBuf {
    dir.join(format!("front_order-{:016x}.rcs", front_order_key(set) as u64))
}
/// The load order the last collected image of a program with these crates was recorded in.
fn read_front_order(dir: &std::path::Path, set: u128) -> Option<Vec<(String, String)>> {
    let store = Store::open(&front_order_path(dir, set))?;
    let (kind, bytes) = store.get(front_order_key(set))?;
    if kind != KIND_FRONT_ORDER { return None; }
    crate::crate_image::decode(bytes)
}
/// Records `recipe`'s order as its crate set's last one (after its collected image is written:
/// the next start loads in this order and restores that image). Unchanged: nothing written.
pub(crate) fn write_front_order(dir: &std::path::Path, recipe: &crate::crate_image::GraphRecipe) {
    if read_front_order(dir, recipe.set).as_ref() == Some(&recipe.crates) { return; }
    let entry = NewEntry { key: front_order_key(recipe.set), kind: KIND_FRONT_ORDER, payload: crate::crate_image::encode(&recipe.crates) };
    let _ = crate::store::append(&front_order_path(dir, recipe.set), vec![entry], true);
}

/// The stored graph a reordered load restores its leading crates from (FrontImages::load_order):
/// the last recorded order of the same crates, of which this load keeps the first `common`.
pub(crate) struct BaseOrder {
    pub(crate) set: u128,
    pub(crate) identity: u128,
    pub(crate) pre: Vec<crate::preload::PreCrate>,
    pub(crate) common: usize,
}
/// Each crate's recorded front record as FrontImages::prefix_records reads it.
pub(crate) type PrefixRecords = Vec<Result<(u128, Vec<String>), &'static str>>;

fn demand_stores_key(identity: u128) -> u128 {
    let mut h = Sip128::new();
    h.str("demand-stores");
    h.u128(front_graph_key(identity));
    h.finish128()
}
fn front_graph_path(dir: &std::path::Path, identity: u128) -> PathBuf {
    dir.join(format!("front_graph-{:016x}.rcs", front_graph_key(identity) as u64))
}
fn read_front_graph(dir: &std::path::Path, identity: u128) -> Option<crate::crate_image::GraphImage> {
    let store = Store::open(&front_graph_path(dir, identity))?;
    let (kind, bytes) = store.get(front_graph_key(identity))?;
    if kind != KIND_FRONT_GRAPH { return None; }
    crate::crate_image::decode(bytes)
}

/// Published after all eager crate stores exist. A stable graph key replaces the
/// old certificate on edits even when an independent Rust-only crate is last.
struct FrontGraphWrite {
    dir: PathBuf,
    options: crate::options::Options,
    recipe: crate::crate_image::GraphRecipe,
    fps: Vec<Option<u128>>,
    /// the load's demand plan: its image is taken on the writer (a plan still planning is not waited for here)
    demand: Option<std::sync::Arc<crate::preload::DemandPlan>>,
    profile: bool,
}
static FRONT_GRAPH_WRITES: relax_session::sync::Mutex<Vec<FrontGraphWrite>> = relax_session::sync::Mutex::new(Vec::new());

impl FrontGraphWrite {
    fn capture(prog: &crate::program::Program, dir: &std::path::Path, fps: &[Option<u128>]) -> Option<Self> {
        let recipe = prog.front_graph.as_ref()?;
        if recipe.crates.len() != prog.crates.len() || recipe.crates.iter().zip(&prog.crates)
            .any(|(key, c)| prog.syms.str(c.name) != key.0 || c.root_path() != key.1) { return None; }
        Some(Self { dir: dir.to_owned(), options: prog.session.options.clone(), recipe: recipe.clone(),
            fps: fps.to_vec(), demand: prog.demand_plan.clone(), profile: prog.session.runtime.profile })
    }
}

/// A boot's crate records and its graph, written after the first present (`flush_front_graphs`, from
/// the first-present hook, the mid-run writer and the exit): the records first (each store is
/// complete before the graph certifies it), then the graph.
struct DeferredFront {
    writers: Option<FrontWriters>,
    graph: Option<FrontGraphWrite>,
}
static DEFERRED_FRONT: relax_session::sync::Mutex<Vec<DeferredFront>> = relax_session::sync::Mutex::new(Vec::new());

/// Starts the writer of the boot's deferred records and graph on its own thread (low priority). Called
/// at the first present; anything not started by then is written by the mid-run writer or the exit.
pub fn start_deferred_front() {
    if DEFERRED_FRONT.lock().unwrap().is_empty() { return; }
    let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-front-graph".into()).spawn(|| { let _z = crate::zone_alloc::compiler_thread();
        crate::os::thread::lower_priority(true);
        let t = std::time::Instant::now();
        let r = flush_front_graphs();
        if crate::preload::trace() {
            eprintln!("makepad-run front: records and graph written after the first present: {:?} in {:.1} ms (t={:.0} ms)", r, t.elapsed().as_secs_f64() * 1e3, crate::clock::ms_since_start());
        }
    });
}

/// One graph writer at a time, for the whole write: a second caller (the exit while the quit
/// or mid-run writer is writing) waits for that write instead of finding no pending job,
/// recording the collected program against a graph not yet written ("graph certificate")
/// and ending the process under the writer.
static FRONT_GRAPH_WRITER: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

fn flush_front_graphs() -> Result<(), String> {
    // Only the cache-writer threads wait here; queueing and pending probes
    // keep their short lock. An empty queue does not mean a mid-run writer
    // has finished the jobs it took. Exit must wait for their demand parts
    // and graph commit before publishing the collected image or terminating.
    let _writer = FRONT_GRAPH_WRITER.lock().unwrap_or_else(|e| e.into_inner());
    for mut d in std::mem::take(&mut *DEFERRED_FRONT.lock().unwrap()) {
        if let Some(w) = d.writers.as_mut() { w.finish(); }
        if let Some(job) = d.graph.take() { FRONT_GRAPH_WRITES.lock().unwrap().push(job); }
    }
    let writes = std::mem::take(&mut *FRONT_GRAPH_WRITES.lock().unwrap());
    let mut writes = writes.into_iter();
    while let Some(job) = writes.next() {
        let recorded = write_front_graph(&job).is_some();
        if job.profile { crate::load_profile::flush_current(); }
        if !recorded {
            // Retain the immutable load snapshot for a later writer/exit attempt.
            let mut pending = FRONT_GRAPH_WRITES.lock().unwrap();
            pending.push(job);
            pending.extend(writes);
            return Err("front graph recording deferred: crate stores unavailable".into());
        }
    }
    Ok(())
}

/// `f` over `items` on the loader's worker count (each takes the next item: sizes differ a
/// lot, std's demand part is many times an app crate's), results in order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: Vec<relax_session::sync::Mutex<Option<R>>> = items.iter().map(|_| relax_session::sync::Mutex::new(None)).collect();
    crate::pool::scope(|scope| {
        for _ in 0..crate::preload::jobs().min(items.len()).max(1) {
            scope.spawn(|| { loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                *out[i].lock().unwrap() = Some(f(item));
            } });
        }
    });
    out.into_iter().map(|r| r.into_inner().unwrap().expect("par_map result")).collect()
}

fn write_front_graph(job: &FrontGraphWrite) -> Option<()> {
    let _profile = crate::load_profile::span(job.profile, "front-graph-write", "graph writer");
    // MAKEPAD_RUN_BOOT_TRACE: where a graph write's time goes (one line per recorded graph)
    let t0 = std::time::Instant::now();
    let mut marks: Vec<(&str, f64)> = Vec::new();
    let mut mark = |name: &'static str| marks.push((name, t0.elapsed().as_secs_f64() * 1e3));
    let (dir, recipe, fps) = (&job.dir, &job.recipe, &job.fps);
    let mut graph = crate::crate_image::GraphImage {
        identity: recipe.identity, crates: Vec::new(), inputs: Vec::new(), stamps: Vec::new(), probes: Vec::new(),
        demand_image: job.demand.as_ref().map(|p| p.image.clone()),
    };
    let mut inputs = std::collections::BTreeMap::new();
    let mut probes = std::collections::BTreeMap::new();
    let crates_span = crate::load_profile::span(job.profile, "front-graph-crates", "graph writer");
    for (ci, key) in recipe.crates.iter().enumerate() {
        let fp = fps.get(ci).copied().flatten()?;
        let store = Store::open(&dir.join(front_store_name(&job.options, &key.0, fp)))?;
        let (kind, bytes) = store.get(front_crate_key(&key.0, &key.1))?;
        if kind != KIND_FRONT_CRATE { return None; }
        let rec = crate::crate_image::decode_record_head(bytes)?;
        if !rec.complete || rec.name != key.0 || rec.root != key.1 { return None; }
        for (path, hash) in rec.inputs.into_iter().chain(rec.files) {
            if inputs.insert(path, hash).is_some_and(|old| old != hash) { return None; }
        }
        for (path, exists) in rec.probes {
            if probes.insert(path, exists).is_some_and(|old| old != exists) { return None; }
        }
        graph.crates.push((key.clone(), fp));
    }
    // Selection reads even skipped files and negative module candidates. Keep
    // that global proof in the graph, not in any crate's input certificate.
    // Replacing this record also refreshes demand when the final crate is clean.
    // (a carried plan's proof is the earlier load's: its parts carry their own inputs)
    if let Some(demand) = graph.demand_image.as_ref().filter(|_| !job.demand.as_ref().is_some_and(|p| p.is_carried())) {
        for (path, hash) in &demand.inputs {
            if inputs.insert(path.clone(), *hash).is_some_and(|old| old != *hash) { return None; }
        }
        for (path, exists) in &demand.probes {
            if probes.insert(path.clone(), *exists).is_some_and(|old| old != *exists) { return None; }
        }
    }
    drop(crates_span);
    mark("crates");
    graph.inputs = inputs.into_iter().collect();
    let _stamps = crate::load_profile::span(job.profile, "front-graph-stamps", "graph writer");
    // each input's stat, taken only where the recorded hash is the file's hash right now and the
    // file has been quiet for a while: validation then trusts an equal stat (see begin_load)
    let now_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    // read and hashed on the loader's worker count (1600 Calculator inputs: 0.2 s on one)
    graph.stamps = par_map(&graph.inputs, |(p, h)| crate::source_context::input_stamp(p, *h, now_ns));
    graph.probes = probes.into_iter().collect();
    drop(_stamps);
    mark("stamps");
    if job.profile { eprintln!("front-graph-inputs: {} inputs, {} probes", graph.inputs.len(), graph.probes.len()); }
    let path = front_graph_path(dir, recipe.identity);
    let mut entries = Vec::new();
    if let Some(demand) = &graph.demand_image {
        // retained parts: this graph's store, or a reordered load's base graph's
        let previous = [Some(path.clone()), recipe.base.map(|b| front_graph_path(dir, b))].into_iter().flatten().filter_map(|p| Store::open(&p)).collect::<Vec<_>>();
        let mut header = (**demand).clone();
        let recorded = { let _r = crate::load_profile::span(job.profile, "front-graph-demand-record", "graph writer"); demand.recorded_parts() };
        mark("demand-record");
        if recorded.len() != demand.parts.len() { return None; }
        let _encode = crate::load_profile::span(job.profile, "rust-demand-encode", "graph writer");
        if job.profile { eprintln!("rust-demand-parts: encoded={} retained={} writer=post-load", recorded.iter().filter(|p| p.key == 0).count(), recorded.iter().filter(|p| p.key != 0).count()); }
        // the changed crates' parts are encoded in parallel, each opening its crate's front-image
        // store (the part references sources/tokens there); unchanged ones keep their row
        let indices: Vec<usize> = (0..recorded.len()).collect();
        let encoded = par_map(&indices, |&ci| -> Option<Option<(u128, Vec<u8>)>> {
            let part = &recorded[ci];
            if part.key != 0 { return Some(None); }
            let name = &recipe.crates[ci].0;
            let mut images = demand::Images::open(dir, front_store_name(&job.options, name, fps[ci]?))?;
            let body = part.encode_body(&mut images, job.profile, name)?;
            images.finish(job.profile, name)?;
            Some(Some(body))
        });
        let mut part_stores = std::collections::BTreeSet::new();
        header.parts = recorded.iter().zip(encoded).map(|(part, body)| {
            let (key, payload) = match body? {
                Some(body) => body,
                None => {
                    let (kind, bytes) = previous.iter().find_map(|st| st.get(part.key))?;
                    if kind != KIND_DEMAND_PART { return None; }
                    (part.key, bytes.to_vec())
                }
            };
            part_stores.insert(crate::preload::DemandPart::image_store(&payload)?);
            entries.push(NewEntry { key, kind: KIND_DEMAND_PART, payload });
            Some(std::sync::Arc::new(part.header(key)))
        }).collect::<Option<Vec<_>>>()?;
        graph.demand_image = Some(std::sync::Arc::new(header));
        let names: String = part_stores.iter().map(|n| format!("{n}\n")).collect();
        entries.push(NewEntry { key: demand_stores_key(recipe.identity), kind: KIND_DEMAND_STORES, payload: names.into_bytes() });
    }
    mark("demand-encode");
    let payload = crate::crate_image::encode(&graph);
    entries.push(NewEntry { key: front_graph_key(recipe.identity), kind: KIND_FRONT_GRAPH, payload });
    let _io = SPILL_IO.write().unwrap_or_else(|e| e.into_inner());
    mark("io-lock");
    let _commit = crate::load_profile::span(job.profile, "front-graph-commit", "graph writer");
    crate::store::append(&path, entries, true).ok()?;
    mark("commit");
    if crate::preload::trace() {
        let mut prev = 0.0;
        let parts: Vec<String> = marks.iter().map(|&(n, t)| { let d = t - prev; prev = t; format!("{}={:.0}", n, d) }).collect();
        eprintln!("makepad-run front: graph recorded ({} crates, {} inputs) in {:.0} ms: {}", graph.crates.len(), graph.inputs.len(), prev, parts.join(" "));
    }
    if let (Some(source), Some(written)) = (&job.demand, &graph.demand_image) { source.image.recording_written(written.parts.clone()); }
    Some(())
}

pub fn front_graph_pending() -> bool {
    !FRONT_GRAPH_WRITES.lock().unwrap().is_empty() || !DEFERRED_FRONT.lock().unwrap().is_empty()
}

#[derive(Default)]
pub struct FrontImages {
    dir: PathBuf,
    hashes: relax_session::sync::Mutex<Option<HashMap<String, Option<u128>>>>,
    /// inputs whose contents this handle read and hashed to validate (an unchanged warm start: 0)
    content_hashes: std::sync::atomic::AtomicU64,
    probes: relax_session::sync::Mutex<Option<HashMap<String, bool>>>,
    stores: relax_session::sync::Mutex<HashMap<(String, String), PathBuf>>,
    certificate: relax_session::sync::Mutex<Option<std::sync::Arc<crate::crate_image::GraphImage>>>,
    /// the begun load's graph as read from its store (begin_load), certified or not: the restore and
    /// the demand plan's lookups read it once
    graph_read: relax_session::sync::Mutex<Option<std::sync::Arc<crate::crate_image::GraphImage>>>,
    begun: relax_session::sync::Mutex<Option<u128>>,
    /// the crates a partial collected restore kept (name, fingerprint): the crates loaded after
    /// them name them as dependencies (restore_collected, full_deps)
    retained: relax_session::sync::Mutex<Vec<(String, u128)>>,
    /// of them, the proc-macro crates (CrateImages::retained_proc_macros)
    retained_proc: relax_session::sync::Mutex<Vec<(String, u128)>>,
    /// a reordered load's stored graph (load_order): the restore keeps its leading crates
    base: relax_session::sync::Mutex<Option<BaseOrder>>,
    /// the records load_order read of graph `.0` (the restore reads them once)
    records: relax_session::sync::Mutex<Option<(u128, PrefixRecords)>>,
    // A failed collected restore must hand its completed plan to ordinary preload.
    pending_plan: relax_session::sync::Mutex<Option<(u128, std::sync::Arc<crate::preload::DemandPlan>)>>,
    /// writes the images of the files this run parses (None: a read-only cache)
    sink: Option<std::sync::Arc<FrontSink>>,
}
impl FrontImages {
    /// Graph `identity` as its store holds it: the begun load's read when it is that graph.
    pub(crate) fn front_graph(&self, identity: u128) -> Option<std::sync::Arc<crate::crate_image::GraphImage>> {
        if let Some(g) = self.graph_read.lock().unwrap().as_ref().filter(|g| g.identity == identity) { return Some(g.clone()); }
        read_front_graph(&self.dir, identity).map(std::sync::Arc::new)
    }
    pub fn open(options: &crate::options::Options) -> Option<Self> {
        // A build script's run (its own small world: std plus the script's crates) keeps no
        // front images: they are read only when a script runs again, which a cached build
        // result makes rare, while they cost a second copy of the std world on disk.
        if !crate::front::enabled() || mode_from_env(options).is_none() || std::env::var_os("MAKEPAD_RUN_BUILD_SCRIPT").is_some() {
            return None;
        }
        let root = crate::dirs::relax_dir();
        pack::ensure_private_dir(&root).ok()?;
        let dir = root.join("cache");
        pack::ensure_private_dir(&dir).ok()?;
        let sink = ColdFront::enabled(options).then(|| FrontSink::for_dir(&dir));
        Some(Self { dir, sink, ..Default::default() })
    }
    /// What writes the images of the files and parts a program parses (Program::front_sink).
    pub fn sink(&self) -> Option<std::sync::Arc<dyn crate::parser::ImageSink>> {
        self.sink.clone().map(|s| s as std::sync::Arc<dyn crate::parser::ImageSink>)
    }
    fn input_current(&self, path: &str, expected: u128) -> bool {
        self.memo_input_hash(path) == Some(expected)
    }
    /// An input's content hash, from the load's validation memo while it is active.
    fn memo_input_hash(&self, path: &str) -> Option<u128> {
        let cached = self.hashes.lock().unwrap().as_ref().and_then(|hashes| hashes.get(path)).copied();
        cached.unwrap_or_else(|| {
            self.content_hashes.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let hash = crate::source_context::input_hash(path);
            if let Some(hashes) = self.hashes.lock().unwrap().as_mut() { hashes.insert(path.to_owned(), hash); }
            hash
        })
    }
    fn probe_current(&self, path: &str, expected: bool) -> bool {
        let cached = self.probes.lock().unwrap().as_ref().and_then(|probes| probes.get(path)).copied();
        let exists = cached.unwrap_or_else(|| {
            let exists = crate::vfs::exists(path);
            if let Some(probes) = self.probes.lock().unwrap().as_mut() { probes.insert(path.to_owned(), exists); }
            exists
        });
        exists == expected
    }
    fn inputs_current(&self, inputs: &[(&str, u128)]) -> bool {
        // A graph certificate normally populated the memo already. Do not start
        // another validation thread group for each crate just to read that memo.
        let cached = self.hashes.lock().unwrap().as_ref().and_then(|hashes| {
            inputs.iter().try_fold(true, |valid, (path, hash)| Some(valid && hashes.get(*path)? == &Some(*hash)))
        });
        cached.unwrap_or_else(|| {
            let chunk = inputs.len().div_ceil(crate::preload::jobs().clamp(1, 8)).max(1);
            crate::pool::scope(|scope| {
                let workers: Vec<_> = inputs.chunks(chunk).map(|v| scope.spawn(move || { v.iter().all(|(p, h)| self.input_current(p, *h)) })).collect();
                workers.into_iter().all(|w| w.join().unwrap_or(false))
            })
        })
    }
    /// `why`: the last check reached (MAKEPAD_RUN_BOOT_TRACE names it when the image is not used)
    /// Crate `c`'s image from the front store at `path`. Only a store whose image is taken is
    /// marked as used by this run (store.rs mark_used): the variants a scan rejects stay unused
    /// (cache.rs prune retires replaced variants).
    fn candidate(&self, options: &crate::options::Options, c: &crate::preload::PreCrate, deps: &[u128], path: &std::path::Path, why: &std::cell::Cell<&'static str>, selection: Option<(u128, &[String])>) -> Option<crate::crate_image::CrateImage> {
        why.set("store open");
        let st = std::sync::Arc::new(Store::open_unmarked(path)?);
        let r = self.candidate_of(options, c, deps, &st, why, selection);
        if r.is_some() {
            st.mark_used();
        }
        r
    }

    fn candidate_of(&self, options: &crate::options::Options, c: &crate::preload::PreCrate, deps: &[u128], st: &std::sync::Arc<Store>, why: &std::cell::Cell<&'static str>, selection: Option<(u128, &[String])>) -> Option<crate::crate_image::CrateImage> {
        use crate::crate_image::{decode, CrateImage, Record};
        let st = st.clone();
        let path = &st.path;
        why.set("no crate record");
        let (kind, b) = st.get(front_crate_key(&c.name, &c.root))?;
        if kind != KIND_FRONT_CRATE {
            return None;
        }
        why.set("record decode");
        let mut rec: Record = decode(b)?;
        why.set("context demand selection");
        if selection.is_some_and(|(plan, deferred)| rec.ahead.demand_plan != plan || rec.ahead.deferred != deferred) {
            if crate::preload::trace() {
                eprintln!("boot-trace: {}: recorded selection {:016x} deferred {:?}, now {:016x} {:?}", c.name, rec.ahead.demand_plan as u64, rec.ahead.deferred, selection.map_or(0, |s| s.0) as u64, selection.map(|s| s.1));
            }
            return None;
        }
        let mut cfg = c.cfg.set.clone();
        cfg.sort();
        let checks: [(&'static str, bool); 9] = [
            ("identity: name", rec.name == c.name),
            ("identity: root", rec.root == c.root),
            ("identity: edition", rec.edition == c.edition),
            ("identity: cfg", rec.cfg == cfg),
            ("identity: proc_macro", rec.proc_macro == c.proc_macro),
            ("identity: dependency names", rec.deps == c.image_dep_names),
            ("identity: dependency fingerprints", rec.dep_fps == deps),
            ("identity: lazy bodies", rec.lazy_auto == crate::parser::lazy_auto()),
            ("identity: parser options", rec.parse_options == crate::crate_image::parse_options(options)),
        ];
        if let Some((w, _)) = checks.iter().find(|x| !x.1) {
            if crate::preload::trace() && w.starts_with("identity: dependency") {
                eprintln!("boot-trace: {}: recorded dependencies {:?}, now {:?}", c.name, rec.deps, c.image_dep_names);
            }
            why.set(w);
            return None;
        }
        why.set("incomplete scan or root");
        if !rec.ahead.complete || rec.files.is_empty() || rec.files[0].path != c.root || rec.ahead.name != c.name || rec.ahead.mods.src != c.root {
            return None;
        }
        why.set("module probes");
        if rec.ahead.probes.iter().any(|(p, exists)| !self.probe_current(p, *exists)) {
            return None;
        }
        // The scan's full input set matters even for files subsequently cfg-stripped by collect.
        // No mod lexer is needed: unchanged bytes plus unchanged path choices close the graph.
        why.set("input files changed");
        let mut inputs = std::collections::BTreeMap::new();
        for (path, hash) in rec.ahead.inputs.iter().map(|(p, h)| (p, *h)).chain(rec.files.iter().map(|f| (&f.path, f.hash))) {
            if inputs.insert(path, hash).is_some_and(|old| old != hash) {
                return None;
            }
        }
        let inputs: Vec<_> = inputs.into_iter().map(|(p, h)| (p.as_str(), h)).collect();
        if !self.inputs_current(&inputs) {
            if crate::preload::trace() {
                let changed: Vec<&str> = inputs.iter().filter(|(p, h)| !self.input_current(p, *h)).map(|(p, _)| *p).take(5).collect();
                eprintln!("boot-trace: {}: changed inputs of {} ({} recorded): {}", c.name, path.display(), inputs.len(), changed.join(", "));
            }
            return None;
        }
        why.set("fingerprint / store name");
        let canon = crate::crate_image::canon_deps(c.image_dep_names.iter().map(|x| x.1.clone()).zip(deps.iter().copied()).collect());
        let fp = crate::crate_image::fingerprint(&c.name, &c.root, c.edition, c.proc_macro, &cfg, rec.file_hashes().into_iter(), &canon, rec.selection_hash);
        if path.file_name()?.to_str()? != front_store_name(options, &c.name, fp) {
            return None;
        }
        why.set("late inputs (include!/env!)");
        let present = std::iter::once(c.name.clone()).collect();
        if !ledger_current(&st, &present, &mut HashMap::new()) {
            return None;
        }
        // Offsets, lengths and kinds are checked here; the payload checksums run in the
        // background after the start (verify_front_images: a mismatch removes the store, the
        // next run parses), as for file images -- checksumming every image here touched all of
        // their pages on the boot path. Pin only after validation.
        // the file images: all in this store, else the record still certifies the fingerprint
        // (dependents restore; this crate is parsed)
        let restored = (|| -> Option<Vec<(String, crate::preload::Preparsed)>> {
            why.set("file image missing");
            let mut entries = HashMap::new();
            for f in &rec.files {
                let main = Cache::front_key(&f.path, f.hash);
                let part = Cache::front_part_key(&f.path, f.hash);
                for key in [main, part] {
                    let ix = st.index.binary_search_by_key(&key, |e| e.key).ok();
                    if key == main && ix.is_none() {
                        return None;
                    }
                    if let Some(ix) = ix {
                        let (kind, _) = st.get_at(ix, key)?;
                        if kind != KIND_FRONT_REF {
                            return None;
                        }
                        entries.insert(key, ix);
                    }
                }
            }
            let mapped = |key| front_file_entry(&st, *entries.get(&key)?, key);
            why.set("file image restore");
            let mut files = Vec::new();
            for mut f in rec.files.drain(..) {
                let (loc, img) = mapped(Cache::front_key(&f.path, f.hash))?;
                let mut pf = crate::front::restore(&mut f, img)?;
                pf.image = Some(loc);
                pf.sink = self.sink();
                if let Some((loc, part)) = mapped(Cache::front_part_key(&f.path, f.hash)) {
                    if !crate::front::apply_part(&f.path, &mut pf, part, &c.cfg, false) {
                        return None;
                    }
                    pf.part.get_mut()?.image = Some(loc);
                }
                if f.prep.idents.iter().any(|&(lo, hi)| lo > hi || hi as usize > pf.src.len()) {
                    return None;
                }
                let path = f.path.clone();
                files.push((
                    path,
                    crate::preload::Preparsed {
                        error: None,
                        parsed: Some((pf, f.used)),
                        prep: Some(f.prep),
                        mod_path: f.module,
                        src_len: f.src_len as usize,
                        verify: crate::front::verify().then_some(crate::front::verify_file),
                    },
                ));
            }
            Some(files)
        })();
        let Some(files) = restored else {
            return Some(CrateImage { selection_hash: rec.selection_hash, fingerprint: fp, ahead: rec.ahead, files: Vec::new(), fp_only: true });
        };
        // Restore the late-input union too: otherwise a warm run's next flush could erase
        // include/env dependencies that no parser needed to read this time.
        replay_front_ledger(&st)?;
        // A macro expansion may have declared a module the textual prescan never saw.
        // Its loaded file still has a complete image and must be submitted to the loader.
        for (path, _) in &files {
            if !rec.ahead.files.contains(path) {
                rec.ahead.files.push(path.clone());
            }
        }
        Some(CrateImage { selection_hash: rec.selection_hash, fingerprint: fp, ahead: rec.ahead, files, fp_only: false })
    }
}
impl FrontImages {
    /// CrateImages::previous_files: the newest record of crate `c` (among its stores) whose
    /// identity is `c`'s -- name, root, edition, cfg, kind, dependencies and their fingerprints,
    /// parser options -- with a complete scan and current late inputs (include!/env!): its files'
    /// images by path, source hash and module path, each taken at most once, with its replay data
    /// (a file recorded without it is parsed). Taking one maps it as a crate image's file is
    /// (its lazily parsed part is not taken: the bodies are parsed on demand in this crate's
    /// scope). The record's late inputs are noted for the crate's next record.
    fn previous_files_of(&self, c: &crate::preload::PreCrate, deps: Option<&[u128]>, proc_fps: Option<u128>) -> Option<crate::crate_image::PreviousFiles> {
        use crate::crate_image::{decode, FileMeta, Record};
        let tr = |path: &std::path::Path, why: &str| if crate::preload::trace() { eprintln!("boot-trace: {}: earlier record {} not taken: {}", c.name, path.display(), why); };
        let deps = deps.and_then(|d| self.full_deps(c, d));
        if deps.is_none() && proc_fps.is_none() {
            tr(&self.dir, "dependency fingerprints");
            return None;
        }
        let options = &c.cfg.session.options;
        let mut cfg = c.cfg.set.clone();
        cfg.sort();
        let prefix = format!("{}-", c.name);
        let paths: Vec<_> = crate::store::stores_with_prefix(&self.dir, &prefix).into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n.len() == prefix.len() + 20)).collect();
        let present: HashSet<String> = std::iter::once(c.name.clone()).collect();
        // every such record's files, the newest record's first (any one at a path and source hash
        // is the same parse)
        let mut stores: Vec<_> = paths.iter().filter_map(|p| { let st = Store::open_unmarked(p); if st.is_none() { tr(p, "open"); } st })
            .map(|st| (st.modified(), std::sync::Arc::new(st))).collect();
        stores.sort_by(|a, b| b.0.cmp(&a.0));
        let mut files: HashMap<(String, u128), (FileMeta, std::sync::Arc<Store>, usize)> = HashMap::new();
        for (_, st) in stores {
            let path = st.path.clone();
            let Some((KIND_FRONT_CRATE, b)) = st.get(front_crate_key(&c.name, &c.root)) else { continue };
            let Some(rec) = decode::<Record>(b) else { tr(&path, "decode"); continue };
            let same = rec.name == c.name && rec.root == c.root && rec.edition == c.edition && rec.cfg == cfg && rec.proc_macro == c.proc_macro
                && rec.deps == c.image_dep_names && (deps.as_ref().is_some_and(|d| rec.dep_fps == *d) || proc_fps.is_some() && rec.proc_fps == proc_fps) && rec.lazy_auto == crate::parser::lazy_auto()
                && rec.parse_options == crate::crate_image::parse_options(options) && rec.ahead.complete;
            if !same {
                if crate::preload::trace() {
                    let checks = [rec.name == c.name, rec.root == c.root, rec.edition == c.edition, rec.cfg == cfg, rec.proc_macro == c.proc_macro, rec.deps == c.image_dep_names,
                        deps.as_ref().is_some_and(|d| rec.dep_fps == *d), proc_fps.is_some() && rec.proc_fps == proc_fps, rec.lazy_auto == crate::parser::lazy_auto(), rec.ahead.complete];
                    eprintln!("boot-trace: {}: earlier record {} not taken: identity {:?} (proc {:?} recorded {:?})", c.name, path.display(), checks, proc_fps.map(|x| x as u64), rec.proc_fps.map(|x| x as u64));
                }
                continue;
            }
            if !ledger_current(&st, &present, &mut HashMap::new()) { tr(&path, "late inputs"); continue; }
            let before = files.len();
            for f in rec.files {
                if f.replay.0.is_empty() || files.contains_key(&(f.path.clone(), f.hash)) { continue; }
                let key = Cache::front_key(&f.path, f.hash);
                let Ok(ix) = st.index.binary_search_by_key(&key, |e| e.key) else { continue };
                if !matches!(st.get_at(ix, key), Some((KIND_FRONT_REF, _))) { continue; }
                files.insert((f.path.clone(), f.hash), (f, st.clone(), ix));
            }
            if files.len() == before { tr(&path, "no file with replay data"); continue; }
            replay_front_ledger(&st)?;
            if crate::preload::trace() { eprintln!("boot-trace: {}: parsed, unchanged files' images from {}", c.name, path.display()); }
        }
        if files.is_empty() { return None; }
        let sink = self.sink();
        let files = std::sync::Mutex::new(files);
        Some(std::sync::Arc::new(move |path: &str, hash: u128, module: &str| {
            let (mut f, st, ix) = files.lock().unwrap().remove(&(path.to_string(), hash))?;
            if f.module != module { return None; }
            let key = Cache::front_key(&f.path, f.hash);
            let (loc, img) = front_file_entry(&st, ix, key)?;
            let mut pf = crate::front::restore(&mut f, img)?;
            pf.image = Some(loc);
            pf.sink = sink.clone();
            if f.prep.idents.iter().any(|&(lo, hi)| lo > hi || hi as usize > pf.src.len()) { return None; }
            let used = crate::macros::MacroUse::from_replay(f.used.local, f.used.uses, &f.replay.0)?;
            // (a store an image is taken from is in use: the cap keeps it)
            st.mark_used();
            Some(crate::preload::Preparsed {
                error: None,
                parsed: Some((pf, used)),
                prep: Some(f.prep),
                mod_path: f.module,
                src_len: f.src_len as usize,
                verify: crate::front::verify().then_some(crate::front::verify_file),
            })
        }))
    }

    /// Crate `c`'s dependency fingerprints in its record's order: `deps` are those of its
    /// dependencies loaded in this batch; the ones a partial collected restore kept are not
    /// in the batch (run.rs drops them from `image_deps`), their fingerprints come from the
    /// restored graph. None: a kept dependency's name is ambiguous.
    fn full_deps(&self, c: &crate::preload::PreCrate, deps: &[u128]) -> Option<Vec<u128>> {
        let kept = self.retained.lock().unwrap();
        if kept.is_empty() { return Some(deps.to_vec()); }
        let mut rest = deps.iter();
        let mut out = Vec::with_capacity(c.image_dep_names.len());
        for (_, name) in &c.image_dep_names {
            let mut m = kept.iter().filter(|(n, _)| n == name);
            match (m.next(), m.next()) {
                (Some(&(_, fp)), None) => out.push(fp),
                (None, _) => out.push(*rest.next()?),
                _ => return None,
            }
        }
        rest.next().is_none().then_some(out)
    }

    fn selected_image(&self, c: &crate::preload::PreCrate, deps: &[u128], selection: Option<(u128, &[String])>) -> Option<crate::crate_image::CrateImage> {
        let deps = &self.full_deps(c, deps)?;
        let options = &c.cfg.session.options;
        let certified = self.stores.lock().unwrap().get(&(c.name.clone(), c.root.clone())).cloned();
        if let Some(path) = certified {
            if let Some(image) = self.candidate(options, c, deps, &path, &std::cell::Cell::new(""), selection) { return Some(image); }
        }
        let prefix = format!("{}-", c.name);
        let paths: Vec<_> = crate::store::stores_with_prefix(&self.dir, &prefix).into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n.len() == prefix.len() + 20)).collect();
        let why = std::cell::Cell::new("no store");
        let mut whys: Vec<String> = Vec::new();
        let mut fp_only = None;
        let r = paths.into_iter().find_map(|p| {
            let r = self.candidate(options, c, deps, &p, &why, selection);
            if r.as_ref().map_or(true, |i| i.fp_only) {
                whys.push(format!("{}: {}", p.file_name().and_then(|n| n.to_str()).unwrap_or("?"), why.get()));
            }
            match r {
                Some(i) if i.fp_only => {
                    fp_only.get_or_insert(i);
                    None
                }
                r => r,
            }
        });
        if r.is_none() && fp_only.is_some() && crate::preload::trace() {
            eprintln!("boot-trace: {}: parsed, fingerprint from its record ({})", c.name, whys.join("; "));
            return fp_only;
        }
        let r = r.or(fp_only);
        if r.is_none() && crate::preload::trace() {
            eprintln!("boot-trace: {}: crate image not used: {}", c.name, if whys.is_empty() { "no store".to_string() } else { whys.join("; ") });
        }
        r
    }
}
impl crate::crate_image::CrateImages for FrontImages {
    fn begin_load(&self, crates: &[crate::preload::PreCrate]) {
        let recipe = crate::crate_image::GraphRecipe::new(crates);
        if *self.begun.lock().unwrap() == Some(recipe.identity) { return; }
        *self.begun.lock().unwrap() = Some(recipe.identity);
        *self.certificate.lock().unwrap() = None;
        *self.hashes.lock().unwrap() = Some(HashMap::new());
        *self.probes.lock().unwrap() = Some(HashMap::new());
        self.stores.lock().unwrap().clear();
        let rejected = |why: &str| if crate::preload::trace() {
            eprintln!("makepad-run front: graph certificate not restored: {}", why);
        };
        let Some(graph) = read_front_graph(&self.dir, recipe.identity).map(std::sync::Arc::new) else { rejected("store"); return; };
        *self.graph_read.lock().unwrap() = Some(graph.clone());
        if graph.identity != recipe.identity || graph.crates.len() != recipe.crates.len()
            || !graph.crates.iter().zip(&recipe.crates).all(|((key, _), expected)| key == expected)
            { rejected("recipe"); return; }
        // the module probes and the inputs' stats: thousands of stat calls, spread over threads
        let jobs = crate::preload::jobs().clamp(1, 8);
        let chunk = graph.probes.len().max(graph.inputs.len()).div_ceil(jobs).max(1);
        if graph.stamps.len() != graph.inputs.len() { rejected("input stamps"); return; }
        let g = &graph;
        let probes_ok = crate::pool::scope(|scope| {
            let workers: Vec<_> = (0..jobs).map(|k| scope.spawn(move || {
                let range = |n: usize| (k * chunk).min(n)..((k + 1) * chunk).min(n);
                let ok = g.probes[range(g.probes.len())].iter().all(|(p, exists)| self.probe_current(p, *exists));
                // An input whose stat is the recorded one is unchanged: its recorded hash goes in
                // the memo and its contents are not read. The others are hashed below.
                let trusted: Vec<_> = g.inputs[range(g.inputs.len())].iter().zip(&g.stamps[range(g.inputs.len())])
                    .filter(|((path, _), stamp)| stamp.is_some() && crate::vfs::file_stat(path) == **stamp).map(|((path, hash), _)| (path.clone(), *hash)).collect();
                (ok, trusted)
            })).collect();
            let mut ok = true;
            for w in workers {
                let (probes, trusted) = w.join().unwrap_or((false, Vec::new()));
                ok &= probes;
                let mut memo = self.hashes.lock().unwrap();
                memo.as_mut().unwrap().extend(trusted.into_iter().map(|(path, hash)| (path, Some(hash))));
            }
            ok
        });
        if !probes_ok {
            if crate::preload::trace() {
                if let Some((p, existed)) = graph.probes.iter().find(|(p, e)| !self.probe_current(p, *e)) {
                    rejected(&format!("probe {} was {}", p, existed));
                }
            }
            return;
        }
        let inputs: Vec<_> = graph.inputs.iter().map(|(p, h)| (p.as_str(), *h)).collect();
        if !self.inputs_current(&inputs) {
            if crate::preload::trace() {
                if let Some((p, _)) = inputs.iter().find(|(p, h)| !self.input_current(p, *h)) {
                    rejected(&format!("input {}", p));
                }
            }
            return;
        }
        let mut stores = self.stores.lock().unwrap();
        for (c, (key, fp)) in crates.iter().zip(&graph.crates) {
            stores.insert(key.clone(), self.dir.join(front_store_name(&c.cfg.session.options, &c.name, *fp)));
        }
        *self.certificate.lock().unwrap() = Some(graph);
        if crates.first().is_some_and(|c| c.cfg.session.runtime.profile) {
            eprintln!("makepad-run front: graph certificate restored ({} crates)", crates.len());
        }
    }
    fn end_load(&self) {
        *self.base.lock().unwrap() = None;
        *self.records.lock().unwrap() = None;
        self.hashes.lock().unwrap().take();
        *self.probes.lock().unwrap() = None;
        self.stores.lock().unwrap().clear();
        *self.certificate.lock().unwrap() = None;
        *self.graph_read.lock().unwrap() = None;
        *self.begun.lock().unwrap() = None;
    }
    fn demand_image(&self, crates: &[crate::preload::PreCrate], graph: u128) -> Option<std::sync::Arc<crate::preload::DemandImage>> {
        let recipe = crate::crate_image::GraphRecipe::new(crates);
        let saved = self.front_graph(recipe.identity)?;
        if saved.identity != recipe.identity || saved.crates.iter().map(|(key, _)| key).ne(recipe.crates.iter()) { return None; }
        let image = saved.demand_image.clone()?;
        if image.graph != graph || image.eager_prefix > crates.len() { return None; }
        let indexed: std::collections::BTreeSet<_> = image.files.iter().map(|f| (f.ci, f.path.as_str())).collect();
        if indexed.iter().any(|(ci, _)| *ci >= crates.len())
            || image.reached.iter().any(|(ci, p)| !indexed.contains(&(*ci, p.as_str())))
            || image.probes.iter().any(|(p, e)| !self.probe_current(p, *e)) {
            if crate::preload::trace() {
                for (p, e) in image.probes.iter().filter(|(p, e)| !self.probe_current(p, *e)) { eprintln!("boot-trace: context demand image: probe {} was {}", p, e); }
            }
            return None;
        }
        let late = image.late_inputs.iter().map(|(c, kind, name, hash)| {
            let content = if *kind == crate::sip::LATE_FILE { crate::vfs::read(name).ok() }
                else { crate::crates::crate_env_named(c, name).map(String::into_bytes) };
            (c, *kind, name, *hash, content)
        }).collect::<Vec<_>>();
        if late.iter().any(|(_, _, _, hash, content)| crate::sip::late_hash(content.as_deref()) != *hash) {
            if crate::preload::trace() { eprintln!("boot-trace: context demand image: late input changed"); }
            return None;
        }
        let inputs = image.inputs.iter().map(|(p, h)| (p.as_str(), *h)).collect::<Vec<_>>();
        if !self.inputs_current(&inputs) {
            if crate::preload::trace() {
                for (p, h) in &image.inputs {
                    if !self.input_current(p, *h) { eprintln!("boot-trace: context demand image: input changed: {}", p); }
                }
            }
            return None;
        }
        for (c, kind, name, _, content) in late { crate::sip::note_late_input(c, kind, name, content.as_deref()); }
        Some(image)
    }
    fn take_demand_plan(&self, crates: &[crate::preload::PreCrate]) -> Option<std::sync::Arc<crate::preload::DemandPlan>> {
        let (identity, plan) = self.pending_plan.lock().unwrap().take()?;
        (identity == crate::crate_image::GraphRecipe::new(crates).identity).then_some(plan)
    }
    fn demand_parts(&self, crates: &[crate::preload::PreCrate], graph: u128) -> Option<crate::crate_image::DemandParts> {
        let recipe = crate::crate_image::GraphRecipe::new(crates);
        // a reordered load (load_order): the base graph's parts of the crates it keeps in place
        // (a crate's part covers it and the crates before it); the others are planned again
        let (identity, graph, common) = match self.front_graph(recipe.identity) {
            Some(_) => (recipe.identity, graph, crates.len()),
            None => {
                let base = self.base.lock().unwrap();
                let base = base.as_ref().filter(|b| b.set == recipe.set)?;
                (base.identity, crate::preload::demand_recipe_identity(&base.pre), base.common)
            }
        };
        let Some(saved) = self.front_graph(identity) else { return None; };
        if saved.identity != identity || saved.crates.len() != crates.len()
            || saved.crates.iter().map(|(key, _)| key).take(common).ne(recipe.crates.iter().take(common)) { return None; }
        let Some(image) = saved.demand_image.clone().filter(|image| image.graph == graph && image.parts.len() == crates.len()) else { return None; };
        let path = front_graph_path(&self.dir, identity);
        let store = Store::open(&path)?;
        let profile = crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let keys = image.parts.iter().zip(crates).take(common).map(|(part, c)| {
            let (kind, bytes) = store.get(part.key)?;
            if kind != KIND_DEMAND_PART { return None; }
            let part = crate::preload::DemandPart::read_header(part.key, bytes)?;
            if let Some((p, _)) = part.probes.iter().find(|(p, e)| !self.probe_current(p, *e)) {
                if profile { eprintln!("rust-demand-part-invalid: {} probe {p}", c.name); }
                return None;
            }
            let inputs = part.inputs.iter().map(|(p, h)| (p.as_str(), *h)).collect::<Vec<_>>();
            if !self.inputs_current(&inputs) {
                if profile { for (p, h) in &inputs { if !self.input_current(p, *h) { eprintln!("rust-demand-part-invalid: {} input {p}", c.name); } } }
                return None;
            }
            let late = part.late_inputs.iter().map(|(c, kind, name, hash)| {
                let content = if *kind == crate::sip::LATE_FILE { crate::vfs::read(name).ok() }
                    else { crate::crates::crate_env_named(c, name).map(String::into_bytes) };
                (c, *kind, name, *hash, content)
            }).collect::<Vec<_>>();
            if let Some((_, kind, name, _, _)) = late.iter().find(|(_, _, _, hash, content)| crate::sip::late_hash(content.as_deref()) != *hash) {
                if profile { eprintln!("rust-demand-part-invalid: {} late {kind} {name}", c.name); }
                return None;
            }
            for (c, kind, name, _, content) in late { crate::sip::note_late_input(c, kind, name, content.as_deref()); }
            Some(part.key)
        }).collect::<Vec<_>>();
        let valid = (0..crates.len()).map(|ci| keys.get(ci).is_some_and(Option::is_some)).collect::<Vec<_>>();
        // Stale parts are comparison evidence only, never reusable input. Keep
        // their old demand-out/export sets to validate an edited local selection.
        let keys = image.parts.iter().enumerate().map(|(ci, p)| (ci < common).then_some(p.key)).collect::<Vec<_>>();
        let path = front_graph_path(&self.dir, identity);
        let crates_profile = crates.first().is_some_and(|c| c.cfg.session.runtime.profile);
        let crate_names = crates.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        let image_dir = self.dir.clone();
        let last = crates.len().saturating_sub(1);
        // (a reordered load takes its new last crate in full: always sound, and nothing waits for
        // the closure, which records what it would defer for the next start)
        let last_selected = common < crates.len() || image.files.iter().filter(|f| f.ci == last).all(|f| image.reached.contains(&(last, f.path.clone())));
        Some(crate::crate_image::DemandParts { valid, last_selected, load: std::sync::Arc::new(move || {
            let _profile = crate::load_profile::span(crates_profile, "rust-demand-parts", "load");
            let store = Store::open(&path);
            let chunk = keys.len().div_ceil(crate::preload::jobs().max(1)).max(1);
            crate::pool::scope(|scope| {
                let handles = keys.chunks(chunk).zip(crate_names.chunks(chunk)).map(|(keys, names)| {
                    let store = &store;
                    let image_dir = &image_dir;
                    scope.spawn(move || { keys.iter().zip(names).map(|(key, name)| {
                        let key = (*key)?;
                        let (kind, bytes) = store.as_ref()?.get(key)?;
                        if kind != KIND_DEMAND_PART { return None; }
                        let mut images = demand::ReadImages::default();
                        let part = crate::preload::DemandPart::read(key, bytes, &mut |name, key| images.get(image_dir, name, key));
                        if crates_profile && part.is_none() { eprintln!("rust-demand-part-invalid: {name} payload decode"); }
                        part.map(std::sync::Arc::new)
                    }).collect::<Vec<_>>() })
                }).collect::<Vec<_>>();
                handles.into_iter().flat_map(|h| h.join().expect("demand part reader")).collect()
            })
        }) })
    }

    fn crate_image(&self, c: &crate::preload::PreCrate, deps: &[u128]) -> Option<crate::crate_image::CrateImage> {
        self.selected_image(c, deps, None)
    }
    fn previous_files(&self, c: &crate::preload::PreCrate, deps: Option<&[u128]>, proc_fps: Option<u128>) -> Option<crate::crate_image::PreviousFiles> {
        self.previous_files_of(c, deps, proc_fps)
    }
    fn retained_proc_macros(&self) -> Vec<(String, u128)> {
        self.retained_proc.lock().unwrap().clone()
    }
    fn crate_image_selected(&self, c: &crate::preload::PreCrate, deps: &[u128], plan: u128, deferred: &[String]) -> Option<crate::crate_image::CrateImage> {
        self.selected_image(c, deps, Some((plan, deferred)))
    }
}
fn replay_front_ledger(st: &Store) -> Option<()> {
    let (kind, b) = st.get(LEDGER_KEY)?;
    if kind != KIND_LEDGER {
        return None;
    }
    let mut d = D { b, p: 0 };
    let n = d.u32()?;
    for _ in 0..n {
        let c = d.str()?;
        let k = d.u8()?;
        let name = d.str()?;
        let _hash = d.u128()?;
        let content = if k == crate::sip::LATE_FILE { crate::vfs::read(name).ok() } else { crate::crates::crate_env_named(c, name).map(String::into_bytes) };
        crate::sip::note_late_input(c, k, name, content.as_deref());
    }
    Some(())
}
impl Cache {
    /// Certify complete boot inputs even when the crate must be parsed again. Ordinary files
    /// shared with the proc-macro host need images in this world's own crate store as well.
    pub fn record_front_crates(&mut self, prog: &crate::program::Program) {
        for ci in 0..self.crate_path.len() {
            let Some(c) = prog.crates.get(ci) else { continue };
            let key = front_crate_key(prog.syms.str(c.name), c.root_path());
            if self.mode == Mode::ReadOnly || self.has_index(&key) || self.pending_keys.contains(&key) {
                continue;
            }
            if let Some(rec) = front_record(prog, ci, |k| self.fps.get(k).copied()) {
                for (_, i, _) in prog.front_files.iter().filter(|(k, _, _)| *k as usize == ci) {
                    let f = &prog.files[*i];
                    let path = &prog.file_paths[*i];
                    let part = f.part.get().and_then(|p| p.image.as_ref());
                    self.push_front_refs(ci as u32, path, f.source_hash(path), f.image.as_ref(), part);
                }
                self.pending_keys.insert(key);
                self.push_pending(ci as u32, NewEntry { key, kind: KIND_FRONT_CRATE, payload: crate::crate_image::encode(&rec) });
            }
        }
    }
}

/// Shared record validation for load-time writes and later cache flushes.
fn front_record(prog: &crate::program::Program, ci: usize, fingerprint: impl Fn(usize) -> Option<u128>) -> Option<crate::crate_image::Record> {
    front_record_from(&FrontRecordView::new(prog, ci, fingerprint)?)
}

/// A completed crate's immutable inputs, independent of Program's growing vectors: metadata
/// and where its files' images are, no parsed arrays. Freeze the shared input ledgers now,
/// not when a worker eventually runs.
struct FrontRecordView {
    session: std::sync::Arc<crate::Session>,
    name: String,
    root: String,
    edition: u16,
    cfg: Vec<String>,
    proc_macro: bool,
    ahead: Vec<u8>,
    probes: Vec<(String, bool)>,
    metadata: Vec<(usize, Vec<u8>)>,
    files: Vec<FrontRecordFile>,
    deps: Option<Vec<(String, String)>>,
    dep_fps: Option<Vec<u128>>,
    fingerprint: Option<u128>,
    selection_hash: u128,
    proc_fps: Option<u128>,
}

struct FrontRecordFile {
    index: usize,
    path: String,
    hash: u128,
    fragment: bool,
    /// the file's image and its lazily parsed part's (None: on the heap)
    image: Option<crate::parser::ImageLoc>,
    part: Option<crate::parser::ImageLoc>,
}

impl FrontRecordView {
    fn new(prog: &crate::program::Program, ci: usize, fingerprint: impl Fn(usize) -> Option<u128>) -> Option<Self> {
        let c = prog.crates.get(ci)?;
        let deps = prog.front_dep_names.get(ci).cloned();
        // each dependency as the crate its (extern alias, crate name) pair binds in c.deps --
        // the crate index the load's fingerprint (stable::loaded_crate_fingerprint) and the
        // preload's image deps use -- not the first crate of that name: a graph can hold two
        // crates of one name (Stage: cfg-if 1.0.4 and 1.0.5), and an alias can be both std's and
        // a renamed dependency's (brotli-decompressor's `alloc` = alloc-no-stdlib). Either made the
        // record's fingerprint differ from the load's every run, so no front graph certificate
        // was ever written and every warm Stage load re-validated its 3,500 files.
        let binds = |alias: &str, name: &str| c.deps.iter().find(|&&(a, k)| prog.syms.str(a) == alias
            && prog.crates.get(k as usize).is_some_and(|d| prog.syms.str(d.name) == name)).map(|&(_, k)| k as usize);
        let dep_fps = deps.as_ref().and_then(|deps|
            deps.iter().map(|(alias, name)| binds(alias, name).and_then(&fingerprint)).collect());
        Some(Self {
            session: prog.session.clone(), name: prog.syms.str(c.name).to_owned(), root: c.root_path().to_owned(),
            edition: c.edition, cfg: c.cfg.set.clone(), proc_macro: c.proc_macro,
            ahead: prog.front_ahead.get(ci).cloned().unwrap_or_default(),
            probes: prog.front_probes.get(ci).cloned().unwrap_or_default(),
            metadata: prog.front_files.iter().filter(|(k, _, _)| *k as usize == ci).map(|(_, i, b)| (*i, b.clone())).collect(),
            files: prog.file_crate.iter().enumerate().filter(|(_, k)| **k as usize == ci).map(|(i, _)| {
                let f = &prog.files[i];
                FrontRecordFile {
                    index: i, path: prog.file_paths[i].clone(), hash: f.source_hash(&prog.file_paths[i]), fragment: prog.front_macro_fragments.contains(&i),
                    image: f.image.clone(), part: f.part.get().and_then(|p| p.image.clone()),
                }
            }).collect(),
            deps, dep_fps, fingerprint: fingerprint(ci), selection_hash: c.context.fingerprint(),
            proc_fps: crate::crate_image::proc_fps(prog.crates.iter().enumerate().filter(|(_, d)| d.proc_macro)
                .map(|(k, d)| (prog.syms.str(d.name).to_owned(), fingerprint(k)))),
        })
    }
}

fn front_record_from(prog: &FrontRecordView) -> Option<crate::crate_image::Record> {
    let _mem = crate::memstat::tag(crate::memstat::Tag::FrontRecord);
    let options = &prog.session.options;
    use crate::crate_image::{decode, FileMeta, Record};
    let name = &prog.name;
    let _profile = crate::load_profile::span(prog.session.runtime.profile, "front-load-record", &name);
    let Some(mut ahead) = decode::<crate::preload::CrateAhead>(&prog.ahead) else { return None };
    ahead.probes.extend(prog.probes.iter().cloned());
    ahead.inputs.sort(); ahead.inputs.dedup();
    ahead.probes.sort(); ahead.probes.dedup();
    let mut files = Vec::new();
    let mut indices = Vec::new();
    let mut valid = true;
    for (i, bytes) in &prog.metadata {
        let Some(f) = decode::<FileMeta>(bytes) else {
            valid = false;
            break;
        };
        let loaded = prog.files.binary_search_by_key(i, |f| f.index).ok().map(|j| &prog.files[j]);
        if loaded.map(|f| &f.path) != Some(&f.path)
            || loaded.map(|pf| pf.hash) != Some(f.hash)
            || files.iter().any(|x: &FileMeta| x.path == f.path)
        {
            valid = false;
            break;
        }
        files.push(f);
        indices.push(*i);
    }
    let actual: Vec<_> = prog.files.iter().filter_map(|f| (!f.fragment).then_some(f.index)).collect();
    let ordered = indices == actual;
    if !ahead.complete || !valid || files.is_empty() || !ordered {
        if crate::preload::trace() {
            let why = if !ahead.complete { "incomplete scan" } else if !valid { "invalid or duplicate file metadata" } else if files.is_empty() { "no files" } else { "loaded files differ from the recorded ones (unknown files)" };
            eprintln!("boot-trace: {}: no crate image recorded: {}", name, why);
            if !ordered {
                for &index in actual.iter().filter(|index| !indices.contains(index)) {
                    let f = &prog.files[prog.files.binary_search_by_key(&index, |f| f.index).unwrap()];
                    eprintln!("boot-trace: {}: unrecorded file {}", name, f.path);
                }
            }
        }
        return None;
    }
    // the dependencies as the next run's preload sees them (not the extern crate aliases
    // the sources add while loading)
    let deps = prog.deps.clone()?;
    let dep_fps = prog.dep_fps.clone()?;
    let mut cfg = prog.cfg.clone();
    cfg.sort();
    let rec = Record {
        name: name.clone(),
        root: prog.root.clone(),
        edition: prog.edition,
        cfg,
        proc_macro: prog.proc_macro,
        deps,
        dep_fps,
        lazy_auto: crate::parser::lazy_auto(),
        parse_options: crate::crate_image::parse_options(options),
        ahead,
        files,
        selection_hash: prog.selection_hash,
        proc_fps: prog.proc_fps,
    };
    let canon = crate::crate_image::canon_deps(rec.deps.iter().map(|x| x.1.clone()).zip(rec.dep_fps.iter().copied()).collect());
    let fp = crate::crate_image::fingerprint(&name, &rec.root, rec.edition, rec.proc_macro, &rec.cfg, rec.file_hashes().into_iter(), &canon, rec.selection_hash);
    if fp != prog.fingerprint? {
        if crate::preload::trace() {
            eprintln!("boot-trace: {}: no crate image recorded: fingerprint differs from the cache's", name);
        }
        return None;
    }
    Some(rec)
}

/// A front image entry of store `st` (index entry `e`, key `key`), mapped for the rest of the
/// process: inline (a demand part's) or a file's image a reference names (front_run.rs). The
/// location is Some for a file's image. `check`: verify the payload checksums now; else
/// structure only here (bounds; the image reader checks every section) and the checksum in the
/// background (verify_front_images).
fn front_entry(st: &std::sync::Arc<Store>, e: usize, key: u128, check: bool) -> Option<(Option<crate::parser::ImageLoc>, &'static [u8])> {
    let (kind, b) = st.get_at(e, key)?;
    if check && !st.verify_at(e) {
        return None;
    }
    match kind {
        KIND_FRONT_REF => {
            let (loc, img) = front_run::resolve(st.path.parent()?, b, check)?;
            Some((Some(loc), img))
        }
        KIND_FRONT => {
            pin_front(st, (!check).then_some(e));
            // SAFETY: the store (and its mapping) is kept alive above for the rest of the
            // process, and a committed payload never changes (store.rs)
            Some((None, unsafe { std::slice::from_raw_parts(b.as_ptr(), b.len()) }))
        }
        _ => None,
    }
}

/// front_entry with both checksums verified first.
fn checked_front_entry(st: &std::sync::Arc<Store>, e: usize, key: u128) -> Option<&'static [u8]> {
    front_entry(st, e, key, true).map(|x| x.1)
}

/// A file's image (a reference entry), with its location.
fn front_file_entry(st: &std::sync::Arc<Store>, e: usize, key: u128) -> Option<(crate::parser::ImageLoc, &'static [u8])> {
    match front_entry(st, e, key, false)? {
        (Some(loc), img) => Some((loc, img)),
        (None, _) => None,
    }
}

/// Load-time writer of crate records: a crate whose fingerprint is known (its dependencies are
/// complete) gets its record and references to its files' images (written and mapped when
/// each file was published) on a bounded worker pool. Writes use the same append and
/// late-input ledger path as mid-run flushes.
#[derive(Default)]
pub struct ColdFront {
    fps: Vec<Option<u128>>,
    pending: Vec<usize>,
    writers: Option<FrontWriters>,
    /// crates whose record was written
    records: usize,
    /// boot: the crate records are written after the first present, not while the load runs
    /// (`finish_mode` hands them over, see `DeferredFront`)
    hold: bool,
}
impl ColdFront {
    pub fn enabled(options: &crate::options::Options) -> bool {
        crate::front::enabled() && matches!(mode_from_env(options), Some(Mode::On | Mode::Verify))
    }

    pub fn loaded(&mut self, prog: &crate::program::Program, krate: u32) {
        if !Self::enabled(&prog.session.options) || !prog.record_front { return; }
        self.hold = true;
        self.loaded_in(prog, krate, crate::dirs::cache_dir());
    }

    fn loaded_in(&mut self, prog: &crate::program::Program, krate: u32, dir: PathBuf) {
        let ci = krate as usize;
        let _profile = crate::load_profile::span(prog.session.runtime.profile, "front-load-crate", prog.syms.str(prog.crates[ci].name));
        self.fps.resize(prog.crates.len(), None);
        if self.fps.get(ci) != Some(&None) { return; }
        // Only a load-completion callback makes a crate eligible: merely having a crate
        // index does not mean its files (or its proc_macro flag) are final yet.
        if !self.pending.contains(&ci) { self.pending.push(ci); }
        loop {
            let mut progress = false;
            let mut i = 0;
            while i < self.pending.len() {
                let ci = self.pending[i];
                if let Some(fp) = crate::stable::loaded_crate_fingerprint(prog, ci, &self.fps) {
                    self.fps[ci] = Some(fp);
                    self.pending.remove(i);
                    self.enqueue(prog, ci, &dir);
                    progress = true;
                } else {
                    i += 1;
                }
            }
            if !progress { break; }
        }
    }

    pub fn finish(&mut self, prog: &mut crate::program::Program) {
        if !Self::enabled(&prog.session.options) || !prog.record_front { return; }
        self.finish_mode(prog, crate::dirs::cache_dir(), true);
    }

    #[cfg(test)]
    fn finish_in(&mut self, prog: &mut crate::program::Program, dir: PathBuf) {
        self.finish_mode(prog, dir, false);
    }

    fn finish_mode(&mut self, prog: &mut crate::program::Program, dir: PathBuf, deferred: bool) {
        let _profile = crate::load_profile::span(prog.session.runtime.profile, "front-load-finish", "main");
        self.fps.resize(prog.crates.len(), None);
        // On-demand/shared crates can miss the driver's completion callbacks; cycles also
        // cannot become ready incrementally. Once loading succeeds, use the authoritative
        // full-program calculation for everything left, just as Cache::open does.
        // A complete incremental set skips the full hash pass; its writes are already
        // queued and normally complete before this final join.
        if self.fps.iter().any(Option::is_none) {
            let remaining: Vec<_> = self.fps.iter().enumerate().filter_map(|(ci, fp)| fp.is_none().then_some(ci)).collect();
            if prog.session.runtime.profile {
                for &ci in &remaining {
                    let c = &prog.crates[ci];
                    let waiting: Vec<_> = c.deps.iter().filter(|(_, k)| *k as usize != ci && self.fps.get(*k as usize).copied().flatten().is_none())
                        .map(|(_, k)| prog.crates.get(*k as usize).map_or("?", |c| prog.syms.str(c.name))).collect();
                    let why = if !self.pending.contains(&ci) { "no completion callback" }
                        else { "dependency pending" };
                    // Buffer diagnostics with the timeline; stderr must not serialize
                    // this measured phase just to explain why a crate is pending.
                    let _profile = crate::load_profile::span(true, "front-record-pending",
                        &format!("{}: {}; waiting for {}", prog.syms.str(c.name), why, waiting.join(", ")));
                }
            }
            let fps = {
                let _profile = crate::load_profile::span(prog.session.runtime.profile, "front-finish-fingerprints", "main");
                crate::stable::crate_fingerprints(prog)
            };
            self.fps = fps.iter().copied().map(Some).collect();
            self.pending.clear();
            for ci in remaining { self.enqueue(prog, ci, &dir); }
        }
        // The boot hands its crate records and the graph over to the writer that runs after the first
        // present (or at a quit): a record is a function of the parse, so a crate is restored for this
        // run's cache open as soon as its record is queued; a run killed before the writer ran leaves
        // no record, which the next run sees as absent (a cold load, never a stale one).
        let mut deferred_writers = None;
        let written = if deferred {
            if let Some(w) = self.writers.take() {
                let queued = w.queued.clone();
                deferred_writers = Some(w);
                queued
            } else { Vec::new() }
        } else {
            let _wait = crate::load_profile::span(prog.session.runtime.profile, "front-record-wait", "main");
            self.writers.take().map_or_else(Vec::new, |mut writers| writers.finish())
        };
        for ci in written {
            // front_restored means a current record, not necessarily a no-parse load.
            prog.front_restored[ci] = true;
            self.records += 1;
        }
        // A partial restore can rebuild its entire suffix from older crate images
        // (for example after reverting an edit), leaving front_ahead empty. The
        // writer still needs this load's fingerprints and whole-plan demand
        // proof. It commits them before the post-present collected certificate.
        let _capture = crate::load_profile::span(prog.session.runtime.profile, "front-load-graph-capture", "main");
        if let Some(job) = FrontGraphWrite::capture(prog, &dir, &self.fps) {
            // The demand plan's record (namespaces, identities, contributions: seconds under
            // load) is prepared now, in the background, not by the first write after present:
            // a run quit soon after present (a quick close, a harness's SIGTERM) left it to the
            // exit, whose graph write then outlived the kill and every relaunch was unrestored.
            // The graph itself is written now too, on that thread: every crate store it
            // certifies was joined above. Waiting for the first write after present lost it
            // to a quit soon after present (the mini's harness: SIGTERM at present + 0.5 s,
            // KILL 5 s later; Stage's exit is busy for longer), and a relaunch without the
            // certificate re-parsed every file although each crate's images were on disk.
            // A write that cannot record yet stays queued for the post-present or exit writer.
            if deferred {
                DEFERRED_FRONT.lock().unwrap().push(DeferredFront { writers: deferred_writers.take(), graph: Some(job) });
            }
            else { write_front_graph(&job); }
        } else if let Some(w) = deferred_writers.take() {
            DEFERRED_FRONT.lock().unwrap().push(DeferredFront { writers: Some(w), graph: None });
        }
    }

    fn enqueue(&mut self, prog: &crate::program::Program, ci: usize, dir: &std::path::Path) {
        let _profile = crate::load_profile::span(prog.session.runtime.profile, "front-record-enqueue", prog.syms.str(prog.crates[ci].name));
        // a crate loaded from its image (or a collected graph): its store has its record and
        // its files' references
        if prog.front_ahead.get(ci).is_some_and(|a| a.is_empty()) {
            if crate::preload::trace() { eprintln!("boot-trace: {}: no crate record: loaded from an image", prog.syms.str(prog.crates[ci].name)); }
            return;
        }
        let Some(view) = FrontRecordView::new(prog, ci, |k| self.fps.get(k).copied().flatten()) else {
            if crate::preload::trace() { eprintln!("boot-trace: {}: no crate record: no view", prog.syms.str(prog.crates[ci].name)); }
            return;
        };
        let path = dir.join(front_store_name(&prog.session.options, &view.name, self.fps[ci].unwrap()));
        let late = crate::sip::late_inputs();
        let marks = front_run::run_marks();
        let hold = self.hold;
        self.writers.get_or_insert_with(|| FrontWriters::new(hold)).enqueue(FrontWrite { ci, view, dir: dir.to_owned(), path, late, marks });
    }

    pub fn report(&self, prog: &crate::program::Program) {
        if !prog.session.options.verbose || !Self::enabled(&prog.session.options) { return; }
        use std::sync::atomic::Ordering::Relaxed;
        use crate::source::{MAPPED_SOURCES, MAPPED_SOURCE_MISMATCH, MAPPED_SOURCE_NOT_FOUND, MAPPED_SOURCE_NO_SNAPSHOT};
        eprintln!("makepad-run front: {} crate records written at load; sources mapped {} mismatch {} not-found {} no-snapshot {}", self.records,
            MAPPED_SOURCES.load(Relaxed), MAPPED_SOURCE_MISMATCH.load(Relaxed), MAPPED_SOURCE_NOT_FOUND.load(Relaxed), MAPPED_SOURCE_NO_SNAPSHOT.load(Relaxed));
    }
}

struct FrontWrite {
    ci: usize,
    view: FrontRecordView,
    dir: PathBuf,
    path: PathBuf,
    late: Vec<(String, u8, String, u128)>,
    /// the images written by the enqueue (the view's references name only those)
    marks: front_run::RunMarks,
}

#[derive(Default)]
struct FrontWriteQueue {
    pending: std::collections::VecDeque<FrontWrite>,
    active: HashSet<PathBuf>,
    closed: bool,
}

/// One bounded group per load, fed without waiting for disk or worker capacity. The queue
/// holds at most one snapshot per eligible crate (metadata and image locations only).
/// Same-path jobs retain submission order; distinct paths use any available worker.
struct FrontWriters {
    queue: std::sync::Arc<(relax_session::sync::Mutex<FrontWriteQueue>, relax_session::sync::Condvar)>,
    workers: Vec<std::thread::JoinHandle<Vec<usize>>>,
    /// held: no worker runs until `start` (the boot's records are written after the first present)
    held: bool,
    /// the crates enqueued (a record is written for each unless its snapshot cannot be recorded)
    queued: Vec<usize>,
}

impl FrontWriters {
    fn new(hold: bool) -> Self {
        let mut writers = Self { queue: std::sync::Arc::new(Default::default()), workers: Vec::new(), held: hold, queued: Vec::new() };
        if !hold { writers.start(); }
        writers
    }

    fn start(&mut self) {
        let low = self.held;
        self.held = false;
        for _ in 0..crate::preload::jobs() {
            let queue = self.queue.clone();
            self.workers.push(std::thread::Builder::new().name("run-front-writer".into()).spawn(move || { let _z = crate::zone_alloc::compiler_thread();
                // the records written after the first present must not slow the first frames
                if low { crate::os::thread::lower_priority(true); }
                let (queue, wake) = &*queue;
                let mut recorded = Vec::new();
                loop {
                    let job = {
                        let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(i) = q.pending.iter().position(|job| !q.active.contains(&job.path)) {
                                let job = q.pending.remove(i).unwrap();
                                q.active.insert(job.path.clone());
                                break job;
                            }
                            if q.closed && q.pending.is_empty() { return recorded; }
                            q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let path = job.path.clone();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || write_front_record(job)));
                    match result {
                        Ok(Ok(Some(ci))) => recorded.push(ci),
                        Ok(Ok(None)) => {}
                        Ok(Err(e)) => eprintln!("makepad-run front: crate record: {}", e),
                        Err(_) => eprintln!("makepad-run front: crate record writer panicked"),
                    }
                    queue.lock().unwrap_or_else(|e| e.into_inner()).active.remove(&path);
                    wake.notify_all();
                }
            }).expect("front record writer"));
        }
    }

    fn enqueue(&mut self, job: FrontWrite) {
        self.queued.push(job.ci);
        self.queue.0.lock().unwrap_or_else(|e| e.into_inner()).pending.push_back(job);
        self.queue.1.notify_one();
    }

    /// The crates whose record was written.
    fn finish(&mut self) -> Vec<usize> {
        if self.held { self.start(); }
        self.queue.0.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        self.queue.1.notify_all();
        let mut recorded = Vec::new();
        for worker in self.workers.drain(..) {
            match worker.join() {
                Ok(cis) => recorded.extend(cis),
                Err(_) => eprintln!("makepad-run front: crate record writer panicked"),
            }
        }
        recorded
    }
}

impl Drop for FrontWriters {
    fn drop(&mut self) {
        // Early load errors/unwinding must not leave detached writers. No write gate is held
        // while awaiting jobs.
        self.finish();
    }
}

/// A crate's references to its files' images and its record (Some(crate): a record was
/// written), in its crate store.
fn write_front_record(job: FrontWrite) -> Result<Option<usize>, String> {
    let FrontWrite { ci, view: prog, dir, path, late, marks } = job;
    let profile = prog.session.runtime.profile;
    let name = prog.name.as_str();
    let _profile = crate::load_profile::span(profile, "front-record-crate", name);
    // Only files with a unique source key can supply an image.
    let mut counts = HashMap::new();
    for f in &prog.files {
        *counts.entry((f.path.as_str(), f.hash)).or_insert(0usize) += 1;
    }
    let mut entries = Vec::new();
    for f in prog.files.iter().filter(|f| counts.get(&(f.path.as_str(), f.hash)) == Some(&1)) {
        if let Some(loc) = &f.image {
            entries.push(NewEntry { key: Cache::front_key(&f.path, f.hash), kind: KIND_FRONT_REF, payload: front_run::encode_ref(loc) });
        }
        if let Some(loc) = &f.part {
            entries.push(NewEntry { key: Cache::front_part_key(&f.path, f.hash), kind: KIND_FRONT_REF, payload: front_run::encode_ref(loc) });
        }
    }
    let rec = {
        let _profile = crate::load_profile::span(profile, "front-record-encode", name);
        front_record_from(&prog)
    };
    let recorded = rec.is_some();
    if crate::preload::trace() { eprintln!("boot-trace: {}: crate record {} to {}", name, if recorded { "written" } else { "not written" }, path.display()); }
    if let Some(rec) = rec {
        entries.push(NewEntry { key: front_crate_key(&rec.name, &rec.root), kind: KIND_FRONT_CRATE, payload: crate::crate_image::encode(&rec) });
    }
    if entries.is_empty() { return Ok(None); }
    let lock = front_store_lock(&path);
    let _store = lock.lock().unwrap_or_else(|e| e.into_inner());
    let guard = SPILL_IO.read().unwrap_or_else(|e| e.into_inner());
    let _profile = crate::load_profile::span(profile, "front-record-write", name);
    // Keep code/old parts, even if the boot record was rejected. Cache::open decides
    // staleness after loading; an early write must not retire still-linkable code.
    Spill { dir, files: vec![(path.clone(), false, entries)], marks }.write_locked(&guard, &late)?;
    Ok(recorded.then_some(ci))
}

#[cfg(test)]
mod cold_tests {
    use super::*;

    fn assert_fingerprints(cold: &ColdFront, prog: &crate::program::Program) {
        assert_eq!(cold.fps, crate::stable::crate_fingerprints(prog).into_iter().map(Some).collect::<Vec<_>>());
    }



    #[test]
    fn warm_validation_is_by_stat_and_racy_edits_are_caught() {
        isolated_test!(warm_validation_is_by_stat_and_racy_edits_are_caught);
        let mut session = crate::Session::default();
        session.options.source_eager = true;
        let sess = &std::sync::Arc::new(session);
        let dir = std::env::temp_dir().join(format!("relax-stat-graph-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let root = dir.join("lib.rs").to_string_lossy().into_owned();
        std::fs::write(&dir.join("Cargo.toml"), "[package]\nname = \"quiet\"\nversion = \"0.1.0\"\n[lib]\npath = \"lib.rs\"\n").unwrap();
        std::fs::write(&root, "mod called; pub use called::value;").unwrap();
        let called = dir.join("called.rs");
        std::fs::write(&called, "pub fn value() -> u32 { 7 }").unwrap();
        let pre = vec![crate::preload::PreCrate {  name: "quiet".into(), root, edition: 2021, cfg,
            deps: vec![], proc_macro: false, image_deps: vec![], image_dep_names: vec![] }];
        let store = std::env::temp_dir().join(format!("relax-stat-graph-cache-{}-{}", std::process::id(), crate::clock::wall_ns()));
        // `sink`: the files' images are written (a load from images maps them anyway)
        let load = |images: Option<&dyn crate::crate_image::CrateImages>, sink: bool| {
            let mut prog = crate::program::Program::new(sess);
            prog.record_front = true;
            if sink { prog.front_sink = Some(FrontSink::for_dir(&store)); }
            prog.preload_images(pre.clone(), images);
            for c in &pre {
                prog.load_crate(&c.name, &c.root, c.edition, c.cfg.clone(), vec![]).unwrap();
            }
            prog.resolve_imports();
            prog
        };
        // the files must be older than the racy window when the graph is recorded
        std::thread::sleep(std::time::Duration::from_millis(2300));
        let mut cold = load(None, true);
        ColdFront::default().finish_in(&mut cold, store.clone());
        let identity = cold.front_graph.as_ref().unwrap().identity;
        let graph = read_front_graph(&store, identity).unwrap();
        assert!(!graph.stamps.is_empty() && graph.stamps.iter().all(|s| s.is_some()), "quiet inputs carry a stat: {:?}", graph.inputs.iter().zip(&graph.stamps).filter(|x| x.1.is_none()).map(|x| &x.0.0).collect::<Vec<_>>());
        let images = FrontImages { dir: store.clone(), ..Default::default() };
        let warm = load(Some(&images), false);
        assert!(warm.files.iter().all(|f| f.src.is_mapped()));
        assert_eq!(images.content_hashes.load(std::sync::atomic::Ordering::Relaxed), 0, "an unchanged warm start hashes no file");
        // same size, mtime restored: the stat alone differs only in ctime (and the inode when
        // an editor replaces the file), so it is still caught
        let mtime = std::fs::metadata(&called).unwrap().modified().unwrap();
        std::fs::write(&called, "pub fn value() -> u32 { 8 }").unwrap();
        std::fs::File::options().write(true).open(&called).unwrap().set_modified(mtime).unwrap();
        let edited = load(Some(&images), false);
        // (the edited file is parsed; the unchanged root may take its earlier image: previous_files)
        let parsed = |p: &crate::program::Program| p.files.iter().zip(&p.file_paths).find(|(_, path)| path.ends_with("called.rs")).is_some_and(|(f, _)| !f.src.is_mapped());
        assert!(parsed(&edited), "an edit with the same size and mtime is detected");
        assert!(images.content_hashes.load(std::sync::atomic::Ordering::Relaxed) > 0);
        // a graph recorded within the racy window of the edit carries no stat for that file
        let mut edited = edited;
        ColdFront::default().finish_in(&mut edited, store.clone());
        let racy = read_front_graph(&store, identity).unwrap();
        let at = racy.inputs.iter().position(|(p, _)| p.ends_with("called.rs")).unwrap();
        assert!(racy.stamps[at].is_none(), "a file changed just before the recording is hashed on every validation");
        std::fs::write(&called, "pub fn value() -> u32 { 9 }").unwrap();
        std::fs::File::options().write(true).open(&called).unwrap().set_modified(mtime).unwrap();
        let again = load(Some(&images), false);
        assert!(parsed(&again), "the racy case: same size, restored mtime");
    }



    #[test]
    fn collected_boot_excludes_runtime_crates_and_later_boot_queries() {
        isolated_test!(collected_boot_excludes_runtime_crates_and_later_boot_queries);
        let mut session = crate::Session::default();
        session.options.rs_lazy = true;
        let sess = &std::sync::Arc::new(session);
        let dir = std::env::temp_dir().join(format!("relax-collected-modules-{}-{}", std::process::id(), crate::clock::wall_ns()));
        let package = dir.join("host");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("Cargo.toml"), "[package]\nname = \"test_crate\"\nversion = \"0.1.0\"\n[lib]\npath = \"lib.rs\"\n").unwrap();
        let root = package.join("lib.rs").to_string_lossy().into_owned();
        std::fs::write(&root, "mod late; pub struct FxIn { pub t: f32, pub kick: f32 } pub fn host_scale(t: f32) -> f32 { t * 2.0 } pub fn main() {}").unwrap();
        std::fs::write(package.join("late.rs"), "pub struct Later { pub value: u64 } pub fn later() -> u64 { 3 }").unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let pre = vec![crate::preload::PreCrate { name: "test_crate".into(), root, edition: 2021, cfg: cfg.clone(),
            deps: vec![],  proc_macro: false, image_deps: vec![], image_dep_names: vec![] }];
        let store = dir.join("cache");
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        prog.front_sink = Some(FrontSink::for_dir(&store));
        prog.preload(pre.clone());
        prog.load_crate(&pre[0].name, &pre[0].root, 2021, cfg.clone(), vec![]).unwrap();
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let deferred = *prog.skipped_files.keys().next().expect("deferred boot module");
        ColdFront::default().finish_in(&mut prog, store.clone());
        let identity = prog.front_graph.as_ref().unwrap().identity;
        let graph = read_front_graph(&store, identity).unwrap();
        assert!(graph.demand_image.is_some());
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut unit = Unit::new(prog, tcx, 0);
        let mut cache = Cache::open_in(&unit, Mode::On, dir.join("collected")).unwrap();
        cache.dir = store.clone();
        cache.crate_path = vec![store.join(front_store_name(&sess.options, "test_crate", cache.fps[0]))];
        let boot_defs = unit.prog.defs.len();
        let boot_types = unit.tcx.tys.len();
        let boot_files = unit.prog.file_paths.clone();
        cache.preserve_collected_boot(&unit.prog, &unit.tcx);
        assert!(cache.collected_boot.is_some());
        assert!(Store::open(&collected::path(&store, identity)).is_none(), "snapshot must not publish before the normal writer");

        // Use the run-time module fixtures and the same append/resolve/collect path as
        // modules::front, without running JIT code or creating a process-global module.
        let append = |unit: &mut Unit, name: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/modules/{name}.rs"));
            let d0 = unit.prog.defs.len();
            let i0 = unit.prog.imports.len();
            let host = unit.prog.crates[0].name;
            let krate = unit.prog.load_crate(name, &path.to_string_lossy(), 2021, cfg.clone(), vec![(host, 0)]).unwrap();
            unit.prog.resolve_imports_from(i0);
            let defs: Vec<_> = (d0..unit.prog.defs.len()).map(|i| DefId(i as u32)).collect();
            assert!(unit.tcx.collect_added(&unit.prog, &defs, true).is_empty());
            assert!(unit.prog.errors.is_empty(), "{:?}", unit.prog.errors);
            let frame = defs.iter().copied().find(|d| unit.prog.syms.str(unit.prog.def(*d).name) == "frame").unwrap();
            unit.tcx.tys.intern(TyKind::FnDef(frame, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
            krate
        };
        assert_eq!(append(&mut unit, "fx_a"), 1);
        assert!(unit.prog.front_graph.is_none(), "single-crate fallback replaces the live preload state");
        // A boot file can be appended after a run-time crate, and its declarations can
        // intern more types. Neither a crate prefix nor a type-count cut describes this.
        let first = unit.prog.load_deferred_module(deferred).unwrap();
        let defs: Vec<_> = (first..unit.prog.defs.len()).map(|i| DefId(i as u32)).collect();
        assert!(unit.tcx.collect_added(&unit.prog, &defs, false).is_empty());
        assert!(defs.iter().all(|d| unit.prog.def(*d).krate == 0));
        cache.preserve_collected_boot(&unit.prog, &unit.tcx);
        assert_eq!(append(&mut unit, "fx_b"), 2);
        assert!(unit.tcx.tys.len() > boot_types);
        let runtime_fn = (boot_defs..unit.prog.defs.len()).map(|i| DefId(i as u32))
            .find(|d| unit.prog.def(*d).krate == 1 && unit.prog.syms.str(unit.prog.def(*d).name) == "frame").unwrap();
        let runtime_slot = unit.fn_id(FnKey::Inst(runtime_fn, vec![]));
        assert!(cache.slot_bytes(&unit, runtime_slot).is_none(), "run-time slots must not enter the boot manifest");
        assert!(matches!(cache.encode_collected(&unit.prog, &unit.tcx, &|| true), CollectedRecord::Yielded));
        cache.record_collected(&unit.prog, &unit.tcx).expect("record boot after run-time modules");
        assert!(matches!(cache.encode_collected(&unit.prog, &unit.tcx, &|| false), CollectedRecord::Current));

        let images = FrontImages { dir: store.clone(), ..Default::default() };
        let (saved, saved_tcx, proof) = images.restore_collected(sess, &pre).expect("restore only the boot");
        assert_eq!(saved.crates.len(), pre.len());
        assert_eq!(saved.defs.len(), boot_defs);
        assert_eq!(saved.file_paths, boot_files);
        assert_eq!(saved_tcx.tys.len(), boot_types);
        assert!(saved_tcx.adts.keys().chain(saved_tcx.impl_self.keys()).all(|d| (d.0 as usize) < boot_defs));
        assert_eq!(saved.boot_files, boot_files.len());
        assert!(saved.skipped_files.contains_key(&deferred));
        let proof = proof.expect("complete boot certificate");
        assert_eq!(proof.ifaces, cache.iface_plain);
        // the stored interfaces are bound-set free: run, check and check --boundary of one program bind
        // different sets and take the same image, folding their own set in (apply_bound)
        let bound = [(0u32, 7u64)];
        assert_ne!(items::apply_bound(&saved, &proof.ifaces, &bound), proof.ifaces);
        assert_eq!(items::apply_bound(&saved, &proof.ifaces, &bound), items::apply_bound(&unit.prog, &cache.iface_plain, &bound));
        assert_eq!(items::apply_bound(&saved, &proof.ifaces, &[]), proof.ifaces);
        assert_eq!(proof.indexes.as_ref().unwrap().stores.len(), pre.len());
        assert!(cache.restore_collected_definitions(&saved, proof.definitions));
        let definitions = cache.mapped_definitions.as_ref().unwrap();
        assert!(definitions.key(DefId(boot_defs as u32)).is_none());
        for i in 0..boot_defs {
            let d = DefId(i as u32);
            assert_eq!(definitions.definition(saved.def(d).krate, definitions.key(d).unwrap()), Some(d));
        }
        let mut warm = Unit::new(saved, saved_tcx, 0);
        assert_eq!(append(&mut warm, "fx_a"), 1, "warm startup loads the module afresh");
        assert_eq!(append(&mut warm, "fx_b"), 2);
        assert_eq!(warm.prog.crates.len(), unit.prog.crates.len());
        assert_eq!(crate::crate_image::encode(&read_front_graph(&store, identity).unwrap()), crate::crate_image::encode(&graph),
            "run-time preload and collected publication must not alter the boot demand certificate");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An edit keeps the collected graph's crates below the first changed crate (mapped from
    /// the program image) and collects that crate and every crate loaded after it again:
    /// the edited crate's dependents, and clean crates that load after it (a program's
    /// proc-macro and build crates load after the app's).
    #[test]
    fn collected_prefix_keeps_crates_below_the_first_changed_one() {
        isolated_test!(collected_prefix_keeps_crates_below_the_first_changed_one);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-collected-prefix-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        // three padding crates stand where std's are (a change among those is never a prefix)
        let names = ["pad0", "pad1", "pad2", "low", "mid", "top", "late"];
        let mut pre = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let package = dir.join(name);
            std::fs::create_dir(&package).unwrap();
            let root = package.join("lib.rs").to_string_lossy().into_owned();
            let (text, deps): (String, Vec<usize>) = match *name {
                "low" => ("pub fn value() -> u32 { 7 }".into(), vec![]),
                "mid" => ("pub fn mid() -> u32 { low::value() as u32 + 1 }".into(), vec![3]),
                "top" => ("pub fn top() -> u32 { mid::mid() + 1 }".into(), vec![4]),
                _ => (format!("pub fn f{i}() -> u32 {{ {i} }}"), vec![]),
            };
            std::fs::write(&root, text).unwrap();
            pre.push(crate::preload::PreCrate {  name: name.to_string(), root, edition: 2021, cfg: cfg.clone(),
                deps: deps.iter().map(|&d| (names[d].to_string(), names[d].to_string())).collect(), proc_macro: false,
                image_deps: deps.clone(), image_dep_names: deps.iter().map(|&d| (names[d].to_string(), names[d].to_string())).collect() });
        }
        let store = dir.join("cache");
        let load = |images: Option<&dyn crate::crate_image::CrateImages>| {
            let mut prog = crate::program::Program::new(sess);
            prog.record_front = true;
            prog.front_sink = Some(FrontSink::for_dir(&store));
            prog.preload_images(pre.clone(), images);
            for c in &pre {
                let deps = c.image_deps.iter().map(|&d| (prog.syms.intern(names[d]), d as u32)).collect();
                prog.load_crate(&c.name, &c.root, c.edition, c.cfg.clone(), deps).unwrap();
            }
            prog.resolve_imports();
            assert!(prog.errors.is_empty(), "{:?}", prog.errors);
            prog
        };
        let images = FrontImages { dir: store.clone(), ..Default::default() };
        // a run that loads with these sources and records its collected graph
        let record = |images: &FrontImages| {
            let mut prog = load(Some(images));
            ColdFront::default().finish_in(&mut prog, store.clone());
            let prog = load(Some(images));
            let mut tcx = crate::tcx::Tcx::new();
            tcx.collect(&prog);
            let unit = Unit::new(prog, tcx, 0);
            let mut cache = Cache::open_in(&unit, Mode::On, dir.join("collected")).unwrap();
            cache.dir = store.clone();
            cache.crate_path = unit.prog.crates.iter().enumerate().map(|(i, c)|
                store.join(front_store_name(&sess.options, unit.prog.syms.str(c.name), cache.fps[i]))).collect();
            cache.record_collected(&unit.prog, &unit.tcx).expect("record collected graph");
        };
        record(&images);
        let (whole, _, proof) = images.restore_collected(sess, &pre).expect("unchanged sources restore");
        assert_eq!(whole.crates.len(), names.len());
        assert!(proof.is_some());
        // a body edit of the top crate: everything below it stays, `top` and the clean `late`
        // after it are collected again
        std::fs::write(&pre[5].root, "pub fn top() -> u32 { mid::mid() + 2 }").unwrap();
        let (kept, _, proof) = images.restore_collected(sess, &pre).expect("prefix restore");
        assert_eq!(kept.crates.len(), 5, "crates below the edited one");
        assert!(proof.is_some_and(|p| p.kept.is_some_and(|(k, _)| k == 5)), "a partial restore keeps only the kept crates' definition tables");
        // the next run after recording the edited sources restores everything again
        record(&images);
        assert_eq!(images.restore_collected(sess, &pre).unwrap().0.crates.len(), names.len());
        // a signature change in a lower crate: it and everything above it are collected again
        std::fs::write(&pre[3].root, "pub fn value() -> u64 { 7 }").unwrap();
        let (kept, _, _) = images.restore_collected(sess, &pre).expect("prefix restore");
        assert_eq!(kept.crates.len(), 3, "only the crates below the changed one");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_order_moves_changed_crates_last() {
        isolated_test!(load_order_moves_changed_crates_last);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-load-order-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        // three padding crates stand where std's are; `late` depends on nothing but loads after `top`
        let names = ["pad0", "pad1", "pad2", "low", "mid", "top", "late"];
        let mut pre = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let package = dir.join(name);
            std::fs::create_dir(&package).unwrap();
            let root = package.join("lib.rs").to_string_lossy().into_owned();
            let (text, deps): (String, Vec<usize>) = match *name {
                "low" => ("pub fn value() -> u32 { 7 }".into(), vec![]),
                "mid" => ("pub fn mid() -> u32 { low::value() as u32 + 1 }".into(), vec![3]),
                "top" => ("pub fn top() -> u32 { mid::mid() + 1 }".into(), vec![4]),
                _ => (format!("pub fn f{i}() -> u32 {{ {i} }}"), vec![]),
            };
            std::fs::write(&root, text).unwrap();
            pre.push(crate::preload::PreCrate {  name: name.to_string(), root, edition: 2021, cfg: cfg.clone(),
                deps: deps.iter().map(|&d| (names[d].to_string(), names[d].to_string())).collect(), proc_macro: false,
                image_deps: deps.clone(), image_dep_names: deps.iter().map(|&d| (names[d].to_string(), names[d].to_string())).collect() });
        }
        let store = dir.join("cache");
        let load = |pre: &[crate::preload::PreCrate], images: &FrontImages| {
            let mut prog = crate::program::Program::new(sess);
            prog.record_front = true;
            prog.front_sink = Some(FrontSink::for_dir(&store));
            prog.preload_images(pre.to_vec(), Some(images));
            for c in pre {
                let deps = c.image_deps.iter().map(|&d| (prog.syms.intern(&pre[d].name), d as u32)).collect();
                prog.load_crate(&c.name, &c.root, c.edition, c.cfg.clone(), deps).unwrap();
            }
            prog.resolve_imports();
            assert!(prog.errors.is_empty(), "{:?}", prog.errors);
            prog
        };
        let images = FrontImages { dir: store.clone(), ..Default::default() };
        // a run that loads in `pre`'s order and records its collected graph (and with it the order)
        let record = |pre: &[crate::preload::PreCrate]| {
            let mut prog = load(pre, &images);
            ColdFront::default().finish_in(&mut prog, store.clone());
            let prog = load(pre, &images);
            let mut tcx = crate::tcx::Tcx::new();
            tcx.collect(&prog);
            let unit = Unit::new(prog, tcx, 0);
            let mut cache = Cache::open_in(&unit, Mode::On, dir.join("collected")).unwrap();
            cache.dir = store.clone();
            cache.crate_path = unit.prog.crates.iter().enumerate().map(|(i, c)|
                store.join(front_store_name(&sess.options, unit.prog.syms.str(c.name), cache.fps[i]))).collect();
            cache.record_collected(&unit.prog, &unit.tcx).expect("record collected graph");
        };
        let ordered = |pre: &[crate::preload::PreCrate]| -> Vec<crate::preload::PreCrate> {
            match images.load_order(pre, 3) { Some(order) => crate::preload::reorder(pre, &order).unwrap(), None => pre.to_vec() }
        };
        let order_names = |pre: &[crate::preload::PreCrate]| pre.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(" ");
        record(&pre);
        // unchanged: the recorded order, everything restored
        let now = ordered(&pre);
        assert_eq!(order_names(&now), order_names(&pre));
        assert_eq!(images.restore_collected(sess, &now).unwrap().0.crates.len(), names.len());
        // an edit of `low`: it and its dependents load last; the restore keeps the crates before it
        std::fs::write(&pre[3].root, "pub fn value() -> u32 { 8 }").unwrap();
        let now = ordered(&pre);
        assert_eq!(order_names(&now), "pad0 pad1 pad2 late low mid top");
        let (kept, _, _) = images.restore_collected(sess, &now).expect("prefix restore");
        assert_eq!(kept.crates.len(), 3, "the crates the stored order keeps in place");
        record(&now);
        // the next edit of `low`: the order holds, and `late` is kept too
        std::fs::write(&pre[3].root, "pub fn value() -> u32 { 9 }").unwrap();
        let again = ordered(&pre);
        assert_eq!(order_names(&again), "pad0 pad1 pad2 late low mid top");
        let (kept, _, _) = images.restore_collected(sess, &again).expect("prefix restore");
        assert_eq!(kept.crates.len(), 4, "every crate the edit cannot reach");
        // a restore of the reordered graph is the same program as loading it
        record(&again);
        let (whole, _, _) = images.restore_collected(sess, &ordered(&pre)).expect("unchanged sources restore");
        assert_eq!(whole.crates.iter().map(|c| whole.syms.str(c.name).to_string()).collect::<Vec<_>>().join(" "), "pad0 pad1 pad2 late low mid top");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cold_forward_dependencies_wait_and_drain() {
        isolated_test!(cold_forward_dependencies_wait_and_drain);
        let sess = &std::sync::Arc::new(crate::Session::default());
        forward_dependencies(sess, false);
    }

    #[test]
    fn cold_forward_dependencies_finish_without_callback() {
        isolated_test!(cold_forward_dependencies_finish_without_callback);
        let sess = &std::sync::Arc::new(crate::Session::default());
        forward_dependencies(sess, true);
    }

    #[test]
    fn cold_self_dependency_finishes_without_blocking_later_crates() {
        isolated_test!(cold_self_dependency_finishes_without_blocking_later_crates);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-cold-cycle-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        prog.front_sink = Some(FrontSink::for_dir(&dir));
        let mut cold = ColdFront::default();
        for name in ["a", "b"] {
            let path = dir.join(format!("{name}.rs")).to_string_lossy().into_owned();
            std::fs::write(&path, "pub fn f() {}").unwrap();
            let krate = prog.load_crate(name, &path, 2021, crate::cfg::CfgSet::host(sess, &[]), vec![]).unwrap();
            if krate == 0 {
                let alias = prog.syms.intern("self_alias");
                prog.crates[0].deps.push((alias, 0));
            }
            cold.loaded_in(&mut prog, krate, dir.clone());
        }
        assert!(cold.pending.is_empty());
        assert!(cold.fps[0].is_some());
        assert!(cold.fps[1].is_some());
        assert_eq!(cold.records, 0);
        // every file is mapped at its publication, before any record
        assert!(prog.files.iter().all(|f| f.src.is_mapped() && f.image.is_some()));
        let fps = crate::stable::crate_fingerprints(&prog);
        assert_eq!(cold.fps[0], Some(fps[0]));
        assert_eq!(cold.fps[1], Some(fps[1]));
        cold.finish_in(&mut prog, dir.clone());
        assert_eq!(cold.fps, fps.into_iter().map(Some).collect::<Vec<_>>());
        assert_fingerprints(&cold, &prog);
        assert!(cold.pending.is_empty());
        assert_eq!(cold.records, 2);
        assert_eq!(prog.front_restored, vec![true; 2]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cold_self_alias_graph_matches_full_fingerprints_before_finish() {
        isolated_test!(cold_self_alias_graph_matches_full_fingerprints_before_finish);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-cold-self-alias-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        prog.front_sink = Some(FrontSink::for_dir(&dir));
        let mut cold = ColdFront::default();
        let names = ["draw", "widgets_core", "family", "app"];
        let sources = [
            "extern crate self as draw; pub fn f() -> u32 { 7 }",
            "extern crate self as widgets_core; extern crate draw as draw_alias; pub fn f() -> u32 { draw_alias::f() }",
            "pub fn f() -> u32 { widgets_core::f() + draw::f() }",
            "pub fn f() -> u32 { family::f() + widgets_core::f() }",
        ];
        let deps: [&[usize]; 4] = [&[], &[0], &[1, 0], &[2, 1]];
        for (ci, name) in names.iter().enumerate() {
            let path = dir.join(format!("{name}.rs")).to_string_lossy().into_owned();
            std::fs::write(&path, sources[ci]).unwrap();
            let deps = deps[ci].iter().map(|&d| (prog.syms.intern(names[d]), d as u32)).collect();
            let krate = prog.load_crate(name, &path, 2021, crate::cfg::CfgSet::host(sess, &[]), deps).unwrap();
            assert_eq!(krate as usize, ci);
            if ci < 2 {
                assert!(prog.crates[ci].deps.iter().any(|&(_, d)| d == krate), "source added its self alias");
            }
            let fps = crate::stable::loaded_crate_fingerprint(&prog, ci, &cold.fps);
            assert_eq!(fps, Some(crate::stable::crate_fingerprints(&prog)[ci]));
            cold.loaded_in(&mut prog, krate, dir.clone());
            assert_fingerprints(&cold, &prog);
            assert!(cold.pending.is_empty(), "{name} and its dependencies queued at completion");
            assert_eq!(cold.records, 0);
        }
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let fps = cold.fps.clone();
        cold.finish_in(&mut prog, dir.clone());
        assert_eq!(cold.fps, fps);
        assert!(prog.files.iter().all(|f| f.src.is_mapped()));
        // every file's crate store refers to its image (with or without a crate record)
        for (i, f) in prog.files.iter().enumerate() {
            let ci = prog.file_crate[i] as usize;
            let st = Store::open(&dir.join(front_store_name(&sess.options, prog.syms.str(prog.crates[ci].name), cold.fps[ci].unwrap()))).unwrap();
            let path = &prog.file_paths[i];
            assert_eq!(st.get(Cache::front_key(path, f.source_hash(path))).map(|x| x.0), Some(KIND_FRONT_REF));
        }
        // Complete consumers before their dependencies too: self aliases do not mask a
        // real missing dependency, and completing the root drains the entire graph.
        let mut again = ColdFront::default();
        for ci in (0..names.len()).rev() {
            again.loaded_in(&mut prog, ci as u32, dir.clone());
            if ci != 0 {
                assert!(again.fps.iter().all(Option::is_none));
            }
        }
        assert_fingerprints(&again, &prog);
        assert!(again.pending.is_empty());
        let fps = cold.fps.clone();
        let records = cold.records;
        cold.finish_in(&mut prog, dir.clone());
        assert_eq!(cold.fps, fps);
        assert_eq!(cold.records, records, "a finished load writes no record twice");
        assert_fingerprints(&cold, &prog);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn forward_dependencies(sess: &std::sync::Arc<crate::Session>, missing_callback: bool) {
        let dir = std::env::temp_dir().join(format!("relax-cold-forward-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        prog.front_sink = Some(FrontSink::for_dir(&dir));
        let mut cold = ColdFront::default();
        for (ci, name) in ["a", "b", "c"].into_iter().enumerate() {
            let path = dir.join(format!("{name}.rs")).to_string_lossy().into_owned();
            std::fs::write(&path, format!("pub fn {name}() -> u32 {{ 7 }}")).unwrap();
            // Exercise load_crate's on-demand preload path too.
            let krate = prog.load_crate(name, &path, 2021, cfg.clone(), vec![]).unwrap();
            assert_eq!(krate as usize, ci);
            if ci < 2 {
                let dep_name = if ci == 0 { "b" } else { "c" };
                let alias = prog.syms.intern("alias");
                let alias2 = prog.syms.intern("alias2");
                // Dependencies discovered during load need not have lower indices. Both
                // aliases must contribute the same single canonical dependency hash.
                prog.crates[ci].deps = vec![(alias, krate + 1), (alias2, krate + 1)];
                prog.front_dep_names[ci] = vec![("alias".into(), dep_name.into())];
                assert_eq!(crate::stable::loaded_crate_fingerprint(&prog, ci, &cold.fps), None);
            } else {
                prog.crates[ci].proc_macro = true;
            }
            if ci < 2 || !missing_callback {
                cold.loaded_in(&mut prog, krate, dir.clone());
            }
            if ci < 2 || missing_callback {
                assert!(cold.fps.iter().all(Option::is_none));
            }
        }
        if !missing_callback {
            // Completing c queues b and then a immediately.
            assert!(cold.pending.is_empty());
            assert_fingerprints(&cold, &prog);
        }
        cold.finish_in(&mut prog, dir.clone());
        assert!(cold.pending.is_empty());
        assert_eq!(cold.records, 3);
        assert_eq!(prog.front_restored, vec![true; 3]);
        assert_fingerprints(&cold, &prog);
        // A load with the same files must follow the same readiness rules.
        let mut again = ColdFront::default();
        again.loaded_in(&mut prog, 0, dir.clone());
        again.loaded_in(&mut prog, 1, dir.clone());
        assert!(again.fps.iter().all(Option::is_none));
        again.loaded_in(&mut prog, 2, dir.clone());
        assert_fingerprints(&again, &prog);
        assert!(again.pending.is_empty());
        // its writers write the three records again; they finish before the folder goes
        again.finish_in(&mut prog, dir.clone());
        assert_eq!(again.records, 3);
        // Repeated completion/finalization must not write a crate's record twice.
        cold.loaded_in(&mut prog, 0, dir.clone());
        cold.finish_in(&mut prog, dir.clone());
        assert_eq!(cold.records, 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cold_load_maps_at_publication_and_restores_warm() {
        isolated_test!(cold_load_maps_at_publication_and_restores_warm);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-cold-front-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.rs").to_string_lossy().into_owned();
        let b = dir.join("b.rs").to_string_lossy().into_owned();
        std::fs::write(&a, "pub fn a() -> u32 { 7 }").unwrap();
        std::fs::write(&b, "pub fn b() -> u32 { 9 }").unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let pre = || vec![
            crate::preload::PreCrate {  name: "a".into(), root: a.clone(), edition: 2021, cfg: cfg.clone(), deps: vec![], proc_macro: false, image_deps: vec![], image_dep_names: vec![] },
            crate::preload::PreCrate {  name: "b".into(), root: b.clone(), edition: 2021, cfg: cfg.clone(), deps: vec![("alias".into(), "a".into())], proc_macro: true, image_deps: vec![0], image_dep_names: vec![("alias".into(), "a".into())] },
        ];
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        prog.front_sink = Some(FrontSink::for_dir(&dir));
        prog.preload_images_bounded(pre(), None, true, None);
        let mut cold = ColdFront::default();
        let ca = prog.load_crate("a", &a, 2021, cfg.clone(), vec![]).unwrap();
        // the file left the heap at its publication
        assert!(prog.files[0].src.is_mapped());
        assert_eq!(crate::front::heap_bytes(&prog.files[0]), 0);
        // A failed commit must not certify a crate record.
        let mut failed = ColdFront::default();
        failed.loaded_in(&mut prog, ca, PathBuf::from(&a)); // a source file, not a directory
        failed.finish_in(&mut prog, PathBuf::from(&a));
        assert_eq!(failed.records, 0);
        assert!(!prog.front_restored[0]);
        cold.loaded_in(&mut prog, ca, dir.clone());
        assert_fingerprints(&cold, &prog);
        let alias = prog.syms.intern("alias");
        let alias2 = prog.syms.intern("alias2");
        let cb = prog.load_crate("b", &b, 2021, cfg.clone(), vec![(alias, ca), (alias2, ca)]).unwrap();
        prog.crates[cb as usize].proc_macro = true;
        cold.loaded_in(&mut prog, cb, dir.clone());
        assert_fingerprints(&cold, &prog);
        cold.finish_in(&mut prog, dir.clone());
        assert_eq!(cold.records, 2);
        assert!(prog.files.iter().all(|f| f.src.is_mapped()));
        let images = FrontImages { dir: dir.clone(), ..Default::default() };
        let mut warm = crate::program::Program::new(sess);
        warm.record_front = true;
        warm.preload_images_bounded(pre(), Some(&images), true, None);
        let wa = warm.load_crate("a", &a, 2021, prog.crates[0].cfg.clone(), vec![]).unwrap();
        let alias = warm.syms.intern("alias");
        let wb = warm.load_crate("b", &b, 2021, prog.crates[1].cfg.clone(), vec![(alias, wa)]).unwrap();
        warm.crates[wb as usize].proc_macro = true;
        // restored from the cold run's images: the same image (run store, key) per file
        assert!(warm.files.iter().all(|f| f.src.is_mapped()));
        assert_eq!(warm.files.iter().map(|f| f.image.clone()).collect::<Vec<_>>(), prog.files.iter().map(|f| f.image.clone()).collect::<Vec<_>>());
        assert_fingerprints(&cold, &warm);
        for (a, b) in prog.files.iter().zip(&warm.files) { crate::front::verify_file(a, b); }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lazy_part_is_mapped_before_publication_and_restored_warm() {
        isolated_test!(lazy_part_is_mapped_before_publication_and_restored_warm);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-cold-part-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs").to_string_lossy().into_owned();
        let text = b"pub fn f(x: u32) -> u32 { let y = x + 3; y * 2 }".to_vec();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("test", &path, "test", &cfg));
        let parse = || {
            let table = std::sync::Arc::new(crate::macros::MacroTable::default());
            let (mut f, _) = crate::parser::parse_owned_ctx(sess, text.clone(), None, 2021,
                Some(crate::macros::MacroScope::new(table)), "test", Some(ctx.clone()), true).unwrap();
            crate::program::prepare_file(&mut f, &cfg);
            assert!(f.lazy.is_some());
            f
        };
        use crate::parser::ImageSink;
        let sink = FrontSink::for_dir(&dir);
        let mut f = parse();
        assert!(sink.file(&path, &mut f));
        f.sink = Some(sink.clone());
        let part = f.part(&cfg).unwrap();
        // published mapped: the part and its tree never held heap arrays a reader could see
        assert!(part.toks.is_mapped() && part.image.is_some());
        assert_eq!(f.ast.part.get().unwrap().exprs.heap_bytes(), 0);
        // a fresh parse of the same file reads the cold part's image to the same part
        let fresh = parse();
        let fresh_part = fresh.part(&cfg).unwrap();
        assert_eq!(format!("{:?}", part.bodies), format!("{:?}", fresh_part.bodies));
        assert_eq!(part.rules_exp, fresh_part.rules_exp);
        assert_eq!(part.body_origin, fresh_part.body_origin);
        assert_eq!(format!("{:?}", &f.ast.part.get().unwrap().exprs[..]), format!("{:?}", &fresh.ast.part.get().unwrap().exprs[..]));
        commit_runs().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod data_tests {
    use super::*;

    static CACHE_UNIT: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

    std::thread_local! {
        // Run a nested lookup while an outer lookup owns its link guard, without
        // requiring process-global JIT execution of a static initializer.
        pub(super) static BEFORE_LINK: std::cell::RefCell<Option<Box<dyn FnOnce(&mut Unit)>>> = Default::default();
    }

    #[test]
    fn round23_shim_keys_roundtrip() {
        isolated_test!(round23_shim_keys_roundtrip);
        use relax_types::instance::{ShimKind, ReifyReason};
        let _unit = CACHE_UNIT.lock().unwrap_or_else(|e| e.into_inner());
        let mut session = crate::Session::default();
        session.options.tier1 = false; session.options.tyck_ahead = false;
        let session = std::sync::Arc::new(session);
        let root = std::env::temp_dir().join(format!("relax-shim-keys-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("lib.rs");
        std::fs::write(&source, "#![no_std] fn method() {} fn owner() { let _ = || 1; let _ = || 2; } extern \"C\" { type Code; } struct Borrowed<'a>(&'a u32); type Alias<'a> = Borrowed<'a>; trait View<'a> {} fn early<'a>(_: &'a u32) where 'a: 'a {}").unwrap();
        let mut prog = crate::program::Program::new(&session);
        prog.load_crate("shim_keys", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        let method = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "method").then_some(DefId(i as u32))).unwrap();
        let code = prog.defs.iter().enumerate().find_map(|(i, d)| (d.kind == DefKind::ForeignTy).then_some(DefId(i as u32))).unwrap();
        let closures: Vec<_> = prog.defs.iter().enumerate().filter_map(|(i, d)| (d.kind == DefKind::Closure).then_some(DefId(i as u32))).collect();
        assert_eq!(closures.len(), 2);
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let mut cache = open_test_cache(&u, &root.join("cache"));
        let callable = u.tcx.tys.intern(TyKind::FnPtr(u.tcx.tys.mk_list(&[u.tcx.tys.usize_]), u.tcx.tys.usize_, crate::types::FnHeader::RUST, 0));
        let mut kinds = vec![ShimKind::VTable(method), ShimKind::FnPtr(method, callable), ShimKind::FnPtrAsPtr(method, callable), ShimKind::FnPtrFromPtr(method, callable)];
        for reason in [None, Some(ReifyReason::FnPtr), Some(ReifyReason::Vtable)] { kinds.push(ShimKind::Reify(method, reason)); }
        for closure in closures { for track_caller in [false, true] { kinds.push(ShimKind::ClosureOnce { call_once: method, closure, track_caller }); } }
        let foreign = u.tcx.tys.intern(TyKind::Foreign(code));
        let pointer = u.tcx.tys.intern(TyKind::Ptr(false, foreign));
        let named = |name| u.prog.defs.iter().enumerate().find_map(|(i, _)| (u.prog.name(DefId(i as u32)) == name).then_some(DefId(i as u32))).unwrap();
        let regions = u.tcx.tys.intern_region_args(&[crate::types::RegionId::ERASED]);
        let borrowed = u.tcx.tys.intern(TyKind::Adt(named("Borrowed"), u.tcx.tys.mk_list(&[]), regions));
        let early = u.tcx.tys.intern(TyKind::FnDef(named("early"), u.tcx.tys.mk_list(&[]), regions));
        let alias = u.tcx.tys.intern(TyKind::Alias(relax_types::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Free { def_id: named("Alias") }, args: u.tcx.tys.mk_list(&[]), regions }));
        let object = u.tcx.tys.intern(TyKind::Dyn(named("View"), crate::types::List::EMPTY, crate::types::List::EMPTY, regions, crate::types::RegionId::ERASED, 0, crate::types::List::EMPTY));
        let reference = u.tcx.tys.intern(TyKind::Ref(false, borrowed, crate::types::RegionId::ERASED));
        let array = u.tcx.tys.intern(TyKind::Array(u.tcx.tys.usize_, u.tcx.tys.const_usize(3)));
        let tuple = u.tcx.tys.intern(TyKind::Tuple(u.tcx.tys.mk_list(&[array, reference])));
        let slice = u.tcx.tys.intern(TyKind::Slice(tuple));
        let mut encoded = Vec::new();
        for kind in kinds {
            let key = FnKey::Shim(crate::jit::ShimInstance { kind, args: vec![callable, pointer, borrowed, early, alias, object, reference, array, tuple, slice] });
            let mut bytes = E::default();
            let mut crates = vec![];
            assert!(cache.enc_fnkey(&u, &key, &mut bytes, &mut crates, 0).is_some());
            // the image's walk of a key finds the crates its encoder named (prelink's key_ok)
            crates.sort_unstable();
            crates.dedup();
            assert_eq!(cache.key_crates(&u, &bytes.0), Some(crates));
            assert!(!encoded.contains(&bytes.0), "colliding shim key");
            encoded.push(bytes.0);
        }
        u.tcx = crate::tcx::Tcx::new();
        for n in 0..23 { u.tcx.tys.const_usize(n); }
        for bytes in encoded {
            let mut decoder = D { b: &bytes, p: 0 };
            let key = unsafe { Linker::new(&mut cache).dec_fnkey(&mut u, &mut decoder, 0) }.unwrap();
            assert_eq!(decoder.p, bytes.len());
            // the image's skip walks exactly the bytes the encoder wrote (a short skip cut thunk keys)
            let mut skipped = D { b: &bytes, p: 0 };
            assert!(image::skip_fnkey(&mut skipped, 0, &mut image::no_atoms).is_some() && skipped.p == bytes.len(), "skip_fnkey stopped at {} of {}", skipped.p, bytes.len());
            let mut again = E::default();
            assert!(cache.enc_fnkey(&u, &key, &mut again, &mut vec![], 0).is_some());
            assert_eq!(bytes, again.0);
            for end in 0..bytes.len() {
                assert!(unsafe { Linker::new(&mut cache).dec_fnkey(&mut u, &mut D { b: &bytes[..end], p: 0 }, 0) }.is_none());
            }
        }
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn round27_cached_native_const_array_pointer_words() {
        isolated_test!(round27_cached_native_const_array_pointer_words);
        let _unit = CACHE_UNIT.lock().unwrap_or_else(|e| e.into_inner());
        let mut session = crate::Session::default();
        session.options.tier1 = false; session.options.tyck_ahead = false;
        let session = std::sync::Arc::new(session);
        let root = std::env::temp_dir().join(format!("relax-array-pointers-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("lib.rs");
        std::fs::write(&source, "#![no_std] const N: usize = 2; fn array() -> [*const u8; N] { loop {} }").unwrap();
        let mut prog = crate::program::Program::new(&session);
        prog.load_crate("array_pointers", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        let def = DefId(prog.defs.iter().position(|d| prog.syms.str(d.name) == "array").unwrap() as u32);
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let u = Unit::new(prog, tcx, 0);
        let ty = u.tcx.fn_sig(&u.prog, def).unwrap().ret;
        let TyKind::Array(_, n) = u.tcx.tys.kind(ty) else { panic!("array return type") };
        assert!(n.try_to_target_usize(&u.tcx.tys).is_none(), "fixture must retain the declared alias");
        let target = u.data(b"target", 1, DataKind::Bytes);
        let sym = u.data_sym(target);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&target.to_ne_bytes());
        bytes.extend_from_slice(&target.to_ne_bytes());
        let address = u.data(&bytes, 8, DataKind::Typed(ty));
        let Some(SymKind::Data(index)) = u.syms.kind(u.data_sym(address).id) else { panic!("data symbol") };
        assert_eq!(crate::syms::obj_pointer_words(&u, index), Some(vec![(0, sym), (8, sym)]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn round27_bound_regions_cache_roundtrip_and_reuse() {
        isolated_test!(round27_bound_regions_cache_roundtrip_and_reuse);
        use crate::types::{RegionId, RegionKind, BoundVar, FnHeader, ClosureArgs};
        let _unit = CACHE_UNIT.lock().unwrap_or_else(|e| e.into_inner());
        let mut session = crate::Session::default();
        session.options.tier1 = false; session.options.tyck_ahead = false;
        let session = std::sync::Arc::new(session);
        let root = std::env::temp_dir().join(format!("relax-bound-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("lib.rs");
        std::fs::write(&source, "#![no_std] fn method<T>() {} fn owner() { let _ = || 1; } struct Borrowed<'a>(&'a u32); type Alias<'a> = Borrowed<'a>; trait View<'a> { type Output; } trait Fn<Args> { type FnOutput; }").unwrap();
        let mut prog = crate::program::Program::new(&session);
        prog.load_crate("bound_keys", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
        let mut tcx = crate::tcx::Tcx::new(); tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let named = |name| u.prog.defs.iter().enumerate().find_map(|(i, _)| (u.prog.name(DefId(i as u32)) == name).then_some(DefId(i as u32))).unwrap();
        let method = named("method");
        let tys = &u.tcx.tys;
        let bound = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
        let outer = tys.intern_region(RegionKind::ReBound(1, BoundVar(1)));
        let reference = tys.intern(TyKind::Ref(false, tys.u32_, bound));
        let erased = tys.intern(TyKind::Ref(false, tys.u32_, RegionId::ERASED));
        let static_ref = tys.intern(TyKind::Ref(false, tys.u32_, tys.intern_region(RegionKind::ReStatic)));
        let hr = tys.intern(TyKind::FnPtr(tys.mk_list(&[reference]), tys.unit, FnHeader::RUST, 1));
        let plain = tys.intern(TyKind::FnPtr(tys.mk_list(&[erased]), tys.unit, FnHeader::RUST, 0));
        let static_fn = tys.intern(TyKind::FnPtr(tys.mk_list(&[static_ref]), tys.unit, FnHeader::RUST, 0));
        assert_eq!(tys.erase_regions(static_fn), plain);
        // Source fn(&u32) has the same late-bound region as for<'a> fn(&'a u32).
        // An already-erased, unbound signature is instead `plain`.
        let vacuous = tys.intern(TyKind::FnPtr(tys.mk_list(&[erased]), tys.unit, FnHeader::RUST, 1));
        let outer_ref = tys.intern(TyKind::Ref(false, tys.u32_, outer));
        let inner = tys.intern(TyKind::FnPtr(tys.mk_list(&[reference, outer_ref]), outer_ref, FnHeader::RUST, 1));
        let nested = tys.intern(TyKind::FnPtr(tys.mk_list(&[]), inner, FnHeader::RUST, 2));
        let regions = tys.intern_region_args(&[bound]);
        let object = tys.intern(TyKind::Dyn(named("View"), tys.mk_list(&[reference]), tys.mk_list(&[(named("Output"), outer_ref, 1)]), regions, outer, 1, crate::types::List::EMPTY));
        let fn_object = tys.intern(TyKind::Dyn(named("Fn"), tys.mk_list(&[tys.intern(TyKind::Tuple(tys.mk_list(&[reference])))]), tys.mk_list(&[(named("FnOutput"), tys.unit, 0)]), crate::types::RegionArgsId::EMPTY, RegionId::ERASED, 1, crate::types::List::EMPTY));
        let owner = named("owner");
        let closure = tys.intern(TyKind::Closure(u.prog.def(owner).file, crate::program::closure_exprs(&u.prog, owner)[0], ClosureArgs {
            parent_args: tys.unit, closure_kind_ty: tys.u8_, closure_sig_as_fn_ptr_ty: hr, tupled_upvars_ty: tys.unit,
        }));
        let borrowed = tys.intern(TyKind::Adt(named("Borrowed"), tys.mk_list(&[]), regions));
        let fndef = tys.intern(TyKind::FnDef(method, tys.mk_list(&[hr]), regions));
        let alias = tys.intern(TyKind::Alias(crate::consts::IsRigid::No, AliasTy { kind: AliasTyKind::Free { def_id: named("Alias") }, args: tys.mk_list(&[]), regions }));
        let ct = tys.intern_const(crate::consts::ConstKind::Alias(crate::consts::IsRigid::No, crate::consts::AliasConst {
            kind: crate::consts::AliasConstKind::Anon { def_id: method }, args: vec![crate::consts::GenericArg::Lifetime(outer)].into(),
        }));
        let constant = tys.intern(TyKind::Const(ct));
        let fixtures = [hr, plain, vacuous, nested, object, fn_object, closure, borrowed, fndef, alias, constant];
        let mut cache = open_test_cache(&u, &root.join("cache"));
        for kind in [RegionKind::ReStatic, RegionKind::ReEarlyParam(0), RegionKind::ReVar(0),
            RegionKind::RePlaceholder { universe: crate::types::UniverseIndex(1), var: BoundVar(0) }, RegionKind::ReError] {
            assert!(cache.enc_region(&u, tys.intern_region(kind), &mut E::default()).is_none());
        }
        assert!(cache.dec_region(&u, &mut D { b: &[2], p: 0 }).is_none());
        let mut encodings = Vec::new();
        let mut slots = Vec::new();
        for ty in fixtures {
            let mut e = E::default();
            assert!(cache.enc_ty(&u, ty, &mut e, &mut vec![], 0).is_some());
            assert!(!encodings.contains(&e.0), "colliding binder keys");
            let mut d = D { b: &e.0, p: 0 };
            assert_eq!(cache.dec_ty(&u, &mut d, 0), Some(ty));
            assert_eq!(d.p, e.0.len());
            for end in 0..e.0.len() {
                assert!(cache.dec_ty(&u, &mut D { b: &e.0[..end], p: 0 }, 0).is_none());
            }
            encodings.push(e.0);
            let id = u.fn_id(FnKey::Inst(method, vec![ty]));
            assert!(cache.lookup(&mut u, id).is_none());
            cache.record(&u, id, &[0xc3], &[], &[]);
            slots.push(id);
        }
        assert_eq!(cache.stats.recorded, fixtures.len() as u64);
        // and the program's item graph (cache/deps.rs), recorded with the first key
        assert_eq!(cache.flush().unwrap(), fixtures.len() + 1);
        drop(cache);
        let mut warm = open_test_cache(&u, &root.join("cache"));
        for id in slots {
            let Some((code, _, _)) = warm.lookup(&mut u, id) else {
                panic!("cold entry must hit after reopening the disk cache");
            };
            assert_eq!(code, [0xc3]);
        }
        assert_eq!(warm.stats.hits, fixtures.len() as u64);
        assert_eq!(warm.stats.link_fail, 0);
        drop(warm);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_const_cache_roundtrip() {
        isolated_test!(native_const_cache_roundtrip);
        use relax_types::consts::{Const, ConstKind, ScalarInt, ValTree};
        let mut session = crate::Session::default();
        // These fixtures serialize types without executing code. Unit::drop's
        // tier-1 shutdown retains a process-wide lock until process exit.
        session.options.tier1 = false;
        session.options.tyck_ahead = false;
        let sess = &std::sync::Arc::new(session);
        let _unit = CACHE_UNIT.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("relax-native-const-cache-{}", std::process::id()));
        let mut u = Unit::new(crate::program::Program::new(sess), crate::tcx::Tcx::new(), 0);
        let mut cache = open_test_cache(&u, &root);
        let tys = &u.tcx.tys;
        let wide_ty = tys.intern(TyKind::Int(crate::program::Prim::U128));
        let wide = Const::new_value(tys, ValTree::from_scalar_int(tys,
            ScalarInt::try_from_uint((1u128 << 127) + 23, 16).unwrap()), wide_ty);
        let byte = Const::new_value(tys, ValTree::from_scalar_int(tys,
            ScalarInt::try_from_uint(17, 1).unwrap()), tys.u8_);
        let aggregate = Const::new_value(tys, ValTree::from_branches(tys, &[wide, byte]),
            tys.intern(TyKind::Tuple(tys.mk_list(&[wide_ty, tys.u8_]))));
        let array = tys.mk_array(tys.u8_, tys.const_usize(u64::MAX));
        let types = [array, tys.intern(TyKind::Const(aggregate))];
        let expected = [crate::stable::type_hash(&u.prog, &u.tcx, array),
            crate::stable::type_hash(&u.prog, &u.tcx, types[1])];
        let mut encodings = Vec::new();
        for ty in types {
            let mut e = E(Vec::new());
            assert!(cache.enc_ty(&u, ty, &mut e, &mut Vec::new(), 0).is_some());
            encodings.push(e.0);
        }
        // Arena indices must not be the persisted representation.
        u.tcx = crate::tcx::Tcx::new();
        for n in 0..19 { u.tcx.tys.const_usize(n); }
        for (i, bytes) in encodings.iter().enumerate() {
            let mut d = D { b: bytes, p: 0 };
            let decoded = cache.dec_ty(&mut u, &mut d, 0).unwrap();
            assert_eq!(d.p, bytes.len());
            assert_eq!(crate::stable::type_hash(&u.prog, &u.tcx, decoded), expected[i]);
            for end in 0..bytes.len() {
                let mut truncated = D { b: &bytes[..end], p: 0 };
                assert!(cache.dec_ty(&mut u, &mut truncated, 0).is_none());
            }
        }
        let error = u.tcx.tys.intern_const(ConstKind::Error);
        assert!(cache.enc_const(&u, error, &mut E(Vec::new()), &mut Vec::new(), 0).is_none());
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_opaque_def_cache_roundtrip() {
        isolated_test!(native_opaque_def_cache_roundtrip);
        let mut session = crate::Session::default();
        // These fixtures serialize types without executing code. Unit::drop's
        // tier-1 shutdown retains a process-wide lock until process exit.
        session.options.tier1 = false;
        session.options.tyck_ahead = false;
        let sess = &std::sync::Arc::new(session);
        let _unit = CACHE_UNIT.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("relax-opaque-def-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("lib.rs");
        std::fs::write(&source, b"#![no_std] trait Tr {} fn owner() -> (impl Tr, impl Tr) { loop {} }").unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("opaque_defs", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), vec![]).unwrap();
        let mut definitions = Vec::new();
        for (i, d) in prog.defs.iter().enumerate() { if d.kind == DefKind::OpaqueTy { definitions.push(DefId(i as u32)); } }
        assert_eq!(definitions.len(), 2);
        let mut u = Unit::new(prog, crate::tcx::Tcx::new(), 0);
        let mut cache = open_test_cache(&u, &root.join("cache"));
        let mut encodings = Vec::new();
        let mut hashes = Vec::new();
        for def_id in definitions {
            let ty = u.tcx.tys.intern(TyKind::Alias(relax_types::consts::IsRigid::No,
                AliasTy { kind: AliasTyKind::Opaque { def_id }, args: u.tcx.tys.mk_list(&[]), regions: crate::types::RegionArgsId::EMPTY }));
            let hash = crate::stable::type_hash(&u.prog, &u.tcx, ty);
            assert!(!hashes.contains(&hash)); hashes.push(hash);
            let mut e = E(Vec::new());
            assert!(cache.enc_ty(&u, ty, &mut e, &mut vec![], 0).is_some());
            assert!(!encodings.contains(&e.0));
            let mut d = D { b: &e.0, p: 0 };
            assert_eq!(cache.dec_ty(&mut u, &mut d, 0), Some(ty));
            assert_eq!(d.p, e.0.len());
            encodings.push(e.0);
        }
        drop(cache); std::fs::remove_dir_all(root).unwrap();
    }

    fn open_test_cache(u: &Unit, root: &std::path::Path) -> Cache {
        // ReadOnly avoids a pruning thread; tests explicitly enable in-memory recording.
        let mut c = Cache::open_in(u, Mode::ReadOnly, root.to_path_buf()).unwrap();
        c.mode = Mode::On;
        c
    }

    #[test]
    fn fnhdr_cache_roundtrip() {
        isolated_test!(fnhdr_cache_roundtrip);
        use crate::types::{Abi, FnHeader, Safety};
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-fnhdr-cache-{}", std::process::id()));
        let mut u = Unit::new(crate::program::Program::new(sess), crate::tcx::Tcx::new(), 0);
        let mut cache = open_test_cache(&u, &root);
        let mut encodings = Vec::new();
        for abi in [Abi::Rust, Abi::RustCall, Abi::C, Abi::CUnwind, Abi::System, Abi::SystemUnwind, Abi::RelaxIntrinsic, Abi::SysV64, Abi::SysV64Unwind, Abi::Win64, Abi::Win64Unwind] {
            for safety in [Safety::Safe, Safety::Unsafe] {
                for c_variadic in [false, true] {
                    let header = FnHeader { safety, abi, c_variadic };
                    let ty = u.tcx.tys.intern(TyKind::FnPtr(u.tcx.tys.mk_list(&[u.tcx.tys.bool_]), u.tcx.tys.unit, header, 0));
                    let mut e = E(Vec::new());
                    assert!(cache.enc_ty(&u, ty, &mut e, &mut Vec::new(), 0).is_some());
                    assert!(!encodings.contains(&e.0));
                    let mut d = D { b: &e.0, p: 0 };
                    assert_eq!(cache.dec_ty(&mut u, &mut d, 0), Some(ty));
                    assert_eq!(d.p, e.0.len());
                    encodings.push(e.0);
                }
            }
        }
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dyn_auto_traits_cache_roundtrip() {
        isolated_test!(dyn_auto_traits_cache_roundtrip);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-dyn-auto-cache-{}", std::process::id()));
        pack::ensure_private_dir(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, "#![no_std] trait Principal {} auto trait First {} auto trait Second {}").unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (principal, first, second) = (def("Principal"), def("First"), def("Second"));
        let mut cache = open_test_cache(&u, &root);
        let mut encodings = Vec::new();
        for autos in [vec![], vec![first], vec![first, second]] {
            let ty = u.tcx.tys.intern(TyKind::Dyn(principal, Default::default(), Default::default(), crate::types::RegionArgsId::EMPTY, crate::types::RegionId::ERASED, 0, u.tcx.tys.mk_list(&autos)));
            let mut e = E(Vec::new());
            assert!(cache.enc_ty(&u, ty, &mut e, &mut Vec::new(), 0).is_some());
            assert!(!encodings.contains(&e.0));
            let mut d = D { b: &e.0, p: 0 };
            assert_eq!(cache.dec_ty(&mut u, &mut d, 0), Some(ty));
            assert_eq!(d.p, e.0.len());
            encodings.push(e.0);
        }
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bound_region_types_cache_roundtrip() {
        isolated_test!(bound_region_types_cache_roundtrip);
        use crate::types::{BoundVar, FnHeader, RegionArgsId, RegionId, RegionKind};
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-bound-cache-{}", std::process::id()));
        pack::ensure_private_dir(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, "#![no_std] trait Principal {} struct Holder<'a>(&'a u8);").unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (principal, holder) = (def("Principal"), def("Holder"));
        let mut cache = open_test_cache(&u, &root);
        let tys = &u.tcx.tys;
        let b0 = tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
        let b1 = tys.intern_region(RegionKind::ReBound(0, BoundVar(1)));
        let r = |m, t, r| tys.intern(TyKind::Ref(m, t, r));
        // `fn(&u8)`, `fn(&u8, &u8)` with one and with two bound regions, `fn(&dyn Principal)`,
        // `fn(Holder<'_>) -> &u8`, and the same shapes unbound (`fn(&'static u8)` erased)
        let dyn_bound = tys.intern(TyKind::Dyn(principal, Default::default(), Default::default(), RegionArgsId::EMPTY, b0, 0, Default::default()));
        let holder_bound = tys.intern(TyKind::Adt(holder, crate::types::List::EMPTY, tys.intern_region_args(&[b0])));
        let shapes = [
            (vec![r(false, tys.u8_, b0)], tys.unit, 1),
            (vec![r(false, tys.u8_, b0), r(true, tys.u8_, b0)], tys.unit, 1),
            (vec![r(false, tys.u8_, b0), r(false, tys.u8_, b1)], tys.unit, 2),
            (vec![r(false, dyn_bound, b0)], tys.unit, 1),
            (vec![holder_bound], r(false, tys.u8_, b0), 1),
            (vec![r(false, tys.u8_, RegionId::ERASED)], tys.unit, 0),
        ];
        let mut encodings = Vec::new();
        for (ps, ret, n) in shapes {
            let ty = u.tcx.tys.intern(TyKind::FnPtr(u.tcx.tys.mk_list(&ps), ret, FnHeader::RUST, n));
            // inside another type too (`Vec<fn(&mut Cx)>`'s glue keys)
            let outer = u.tcx.tys.intern(TyKind::Tuple(u.tcx.tys.mk_list(&[ty, u.tcx.tys.bool_])));
            for t in [ty, outer] {
                let mut e = E(Vec::new());
                assert!(cache.enc_ty(&u, t, &mut e, &mut Vec::new(), 0).is_some());
                assert!(!encodings.contains(&e.0));
                let mut d = D { b: &e.0, p: 0 };
                assert_eq!(cache.dec_ty(&mut u, &mut d, 0), Some(t));
                assert_eq!(d.p, e.0.len());
                encodings.push(e.0);
            }
        }
        drop(cache);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn front_record_requires_own_store() {
        isolated_test!(front_record_requires_own_store);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-front-store-{}", std::process::id()));
        let u = Unit::new(crate::program::Program::new(sess), crate::tcx::Tcx::new(), 0);
        let mut c = open_test_cache(&u, &root);
        c.crate_path = vec![root.join("first.rcs"), root.join("second.rcs")];
        let path = "same.rs";
        let pf = crate::parser::parse_owned(sess, b"fn value() {}".to_vec(), 2021).unwrap();
        let hash = pf.source_hash(path);
        let key = Cache::front_key(path, hash);
        let loc = crate::parser::ImageLoc { store: "front_run-0000000000000000.rcs".into(), key: 7 };
        assert_eq!(c.push_front_refs(0, path, hash, Some(&loc), None), 1);
        assert_eq!(c.push_front_refs(0, path, hash, Some(&loc), None), 0, "queued once");
        assert!(c.has_front_key_in_crate(0, key));
        assert!(!c.has_front_key_in_crate(1, key), "another crate's pending reference is insufficient");

        let entries = c.pending.drain(..).map(|(_, e)| e).collect();
        crate::store::append(&c.crate_path[0], entries, true).unwrap();
        let st = std::sync::Arc::new(Store::open(&c.crate_path[0]).unwrap());
        let ix = st.index.binary_search_by_key(&key, |e| e.key).unwrap();
        c.index.insert(key, (0, ix as u32));
        c.stores.push(st);
        assert!(c.has_front_key_in_crate(0, key));
        assert!(!c.has_front_key_in_crate(1, key), "another crate's indexed reference is insufficient");
        c.push_pending(1, NewEntry { key, kind: KIND_FRONT_CRATE, payload: Vec::new() });
        assert!(!c.has_front_key_in_crate(1, key), "the entry must be KIND_FRONT_REF");
        assert_eq!(c.push_front_refs(1, path, hash, Some(&loc), None), 1);
        assert!(c.has_front_key_in_crate(1, key));
        drop(c);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owner_verdict_rejects_body_and_dependency_edits() {
        isolated_test!(owner_verdict_rejects_body_and_dependency_edits);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (root, mut prog, _) = front_fixture(sess, "owner-verdict");
        let owner = DefId(prog.defs.iter().position(|d| d.krate == 2 && d.kind == DefKind::Fn).unwrap() as u32);
        let snapshot = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, None);
        let (key, stored) = snapshot.lookup(&prog, owner, 0);
        assert_eq!(stored, None);
        let key = key.unwrap();
        let path = root.join("verdicts.rcs");
        // an Unchecked verdict (the check completed without certifying the body) round-trips too
        let unchecked = StoredVerdict::Unchecked("unresolved trait property".into());
        let upath = root.join("unchecked.rcs");
        crate::store::append(&upath, vec![NewEntry { key, kind: KIND_BORROWCK, payload: enc_verdict(&unchecked) }], true).unwrap();
        let ustores = vec![std::sync::Arc::new(Store::open(&upath).unwrap())];
        let snapshot = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, ustores.first().cloned());
        assert_eq!(snapshot.lookup(&prog, owner, 0), (Some(key), Some(unchecked)));
        drop(ustores);
        crate::store::append(&path, vec![NewEntry { key, kind: KIND_BORROWCK, payload: enc_verdict(&StoredVerdict::Clean) }], true).unwrap();
        let stores = vec![std::sync::Arc::new(Store::open(&path).unwrap())];
        let snapshot = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, stores.first().cloned());
        assert_eq!(snapshot.lookup(&prog, owner, 0), (Some(key), Some(StoredVerdict::Clean)));
        // a changed interface fingerprint (e.g. another set of bound crates under the owner's crate) never reuses it
        let mut other = vec![0u128; prog.crates.len()];
        other[2] = 1;
        let (k2, v2) = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), other, 0, stores.first().cloned()).lookup(&prog, owner, 0);
        assert_ne!(k2, Some(key));
        assert_eq!(v2, None);
        // another epoch (the program was collected again) or another compiler build never reuses it
        assert_eq!(snapshot.lookup(&prog, owner, 1), (None, None));
        assert_eq!(OwnerVerdicts::new(43, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, stores.first().cloned()).lookup(&prog, owner, 0).1, None);
        // Change a dependency body while preserving all paths and definition ids.
        let file = prog.defs.iter().find(|d| d.krate == 0 && d.kind == DefKind::Fn).unwrap().file as usize;
        prog.files[file] = std::sync::Arc::new(crate::parser::parse_owned(sess, b"pub fn value() -> u32 { 9 }".to_vec(), 2021).unwrap());
        let snapshot = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, stores.first().cloned());
        let (changed, stored) = snapshot.lookup(&prog, owner, 0);
        assert_ne!(changed, Some(key));
        assert_eq!(stored, None);
        // An edit to the owner itself also invalidates the verdict.
        let file = prog.def(owner).file as usize;
        prog.files[file] = std::sync::Arc::new(crate::parser::parse_owned(sess, b"pub fn value() -> u32 { 11 }".to_vec(), 2021).unwrap());
        let snapshot = OwnerVerdicts::new(42, crate::stable::crate_fingerprints(&prog), vec![0; prog.crates.len()], 0, stores.first().cloned());
        let (edited, stored) = snapshot.lookup(&prog, owner, 0);
        assert_ne!(edited, changed);
        assert_eq!(stored, None);
        drop(stores);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bound_crates_mark_only_the_interfaces_that_reach_them() {
        isolated_test!(bound_crates_mark_only_the_interfaces_that_reach_them);
        let sess = &std::sync::Arc::new(crate::Session::default());
        // crates: 0 macro_impl <- 1 mac <- 2 dependent (each depends on the one before)
        let (root, prog, _) = front_fixture(sess, "bound-ifaces");
        let digests = items::file_digests(&prog);
        let fps = crate::stable::crate_fingerprints(&prog);
        let plain = items::crate_ifaces(&prog, &fps, &digests);
        let with = |b: &[(u32, u64)]| items::apply_bound(&prog, &plain, b);
        assert_eq!(with(&[]), plain);
        // crate 1 bound: it and what depends on it (crate 2) change; crate 0, below it, does not
        let b1 = with(&[(1, 99)]);
        assert_eq!(b1[0], plain[0]);
        assert_ne!(b1[1], plain[1]);
        assert_ne!(b1[2], plain[2]);
        // crate 2 bound: only itself
        let b2 = with(&[(2, 99)]);
        assert_eq!((b2[0], b2[1]), (plain[0], plain[1]));
        assert_ne!(b2[2], plain[2]);
        // another source identity of the bound crate differs
        assert_ne!(with(&[(1, 100)])[2], b1[2]);
        // the boundary-crossing set is global: crate 1 bound in two programs, crate 2 (which depends on
        // it) bound in only one of them -- crate 1's types cross or not depending on crate 2's fns, so
        // crate 1's fingerprint (and everything above it) must differ, while crate 0 below stays shared
        let (only1, both) = (with(&[(1, 99)]), with(&[(1, 99), (2, 5)]));
        assert_eq!(only1[0], both[0]);
        assert_ne!(only1[1], both[1], "a bound crate depending on another changes that one's view");
        assert_ne!(only1[2], both[2]);
        // order of the list does not matter
        assert_eq!(with(&[(1, 99), (2, 5)]), with(&[(2, 5), (1, 99)]));
        // a bound crate that nothing reaches does not touch an unrelated crate: crate 0 is below all
        assert_eq!(with(&[(2, 5)])[0], plain[0]);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn front_fixture(sess: &std::sync::Arc<crate::Session>, tag: &str) -> (PathBuf, crate::program::Program, Vec<crate::preload::PreCrate>) {
        let root = std::env::temp_dir().join(format!("relax-front-{tag}-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let cfg = crate::cfg::CfgSet::host(sess, &[]);
        let mut prog = crate::program::Program::new(sess);
        prog.record_front = true;
        // the cache folder of open_test_cache(.., &root): files are mapped at publication
        prog.front_sink = Some(FrontSink::for_dir(&root.join("cache")));
        let mut pre: Vec<crate::preload::PreCrate> = Vec::new();
        for (ci, name) in ["macro_impl", "mac", "dependent"].into_iter().enumerate() {
            let path = root.join(format!("{name}.rs")).to_string_lossy().into_owned();
            std::fs::write(&path, "pub fn value() -> u32 { 7 }").unwrap();
            let deps: Vec<(String, String)> = if ci == 0 { vec![] } else { vec![(pre[ci - 1].name.clone(), pre[ci - 1].name.clone())] };
            pre.push(crate::preload::PreCrate {
                name: name.into(), root: path, edition: 2021, cfg: cfg.clone(), deps: deps.clone(), proc_macro: ci == 1,
                image_deps: if ci == 0 { vec![] } else { vec![ci - 1] }, image_dep_names: deps,
            });
        }
        // recorded as a cold run records: the whole graph preloaded under its demand plan, so a
        // warm load (which plans the same graph) selects the same parts of each crate
        prog.preload_images_bounded(pre.clone(), None, true, None);
        for (ci, c) in pre.iter().enumerate() {
            let loaded_deps = c.deps.iter().map(|(alias, _)| (prog.syms.intern(alias), ci as u32 - 1)).collect();
            prog.load_crate(&c.name, &c.root, 2021, cfg.clone(), loaded_deps).unwrap();
            prog.front_dep_names[ci] = c.deps.clone();
            prog.crates[ci].proc_macro = c.proc_macro;
        }
        (root, prog, pre)
    }

    #[test]
    fn front_record_writes_host_files_into_own_stores() {
        isolated_test!(front_record_writes_host_files_into_own_stores);
        let options = &crate::options::Options::default();
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (root, mut prog, pre) = front_fixture(sess, "host");
        // Files parsed again (lazily) and published with their images: their lazy parts are
        // written and mapped at publication too, and travel with the main images.
        use crate::parser::ImageSink;
        let sink = FrontSink::for_dir(&root.join("cache"));
        for i in 0..2 {
            let path = prog.file_paths[i].clone();
            let cfg = prog.crates[i].cfg.clone();
            let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&pre[i].name, &path, &pre[i].name, &cfg));
            let (mut pf, used) = crate::parser::parse_owned_ctx(sess, std::fs::read(&path).unwrap(), None, 2021,
                Some(crate::macros::MacroScope::new(prog.macros.clone())), &pre[i].name, Some(ctx), true).unwrap();
            let prep = crate::program::prepare_file(&mut pf, &cfg);
            prog.front_files[i].2 = crate::crate_image::file_meta(&path, &pre[i].name, &pf, &used.unwrap(), &prep, false);
            assert!(sink.file(&path, &mut pf));
            pf.sink = Some(sink.clone());
            prog.files[i] = std::sync::Arc::new(pf);
            assert!(prog.files[i].part(&cfg).unwrap().toks.is_mapped());
        }
        let u = Unit::new(prog, crate::tcx::Tcx::new(), 0);
        let mut c = open_test_cache(&u, &root);
        c.record_front_crates(&u.prog);
        let pending = c.pending.len();
        c.record_front_crates(&u.prog);
        assert_eq!(c.pending.len(), pending, "recording is idempotent");
        c.spill_take().unwrap().write().unwrap();
        let images = FrontImages { dir: c.dir.clone(), ..Default::default() };
        for (ci, p) in pre.iter().enumerate() {
            let deps: Vec<_> = p.image_deps.iter().map(|&d| c.fps[d]).collect();
            let why = std::cell::Cell::new("");
            let image = images.candidate(options, p, &deps, &c.crate_path[ci], &why, None).unwrap_or_else(|| panic!("{}: {}", p.name, why.get()));
            assert!(!image.fp_only, "{}", p.name);
            assert_eq!(image.fingerprint, c.fps[ci]);
            let restored = &image.files[0].1.parsed.as_ref().unwrap().0;
            crate::front::verify_file(&u.prog.files[ci], restored);
            if ci < 2 { assert!(restored.part.get().unwrap().toks.is_mapped()); }
        }
        // Identical length is insufficient: even one changed source byte rejects the record.
        std::fs::write(&pre[0].root, "pub fn value() -> u32 { 8 }").unwrap();
        let why = std::cell::Cell::new("");
        assert!(images.candidate(options, &pre[0], &[], &c.crate_path[0], &why, None).is_none());
        assert_eq!(why.get(), "input files changed");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The exit writes references through Cache::flush: another process must find every image
    /// they name in the run store's committed index (read from disk, as a new process would).
    #[test]
    fn flushed_references_name_committed_images() {
        isolated_test!(flushed_references_name_committed_images);
        use crate::parser::ImageSink;
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (root, mut prog, pre) = front_fixture(sess, "flush");
        let sink = FrontSink::for_dir(&root.join("cache"));
        // a lazily parsed file whose part is written after the load, like a body parsed at run time
        let path = prog.file_paths[0].clone();
        let cfg = prog.crates[0].cfg.clone();
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&pre[0].name, &path, &pre[0].name, &cfg));
        let (mut pf, used) = crate::parser::parse_owned_ctx(sess, std::fs::read(&path).unwrap(), None, 2021,
            Some(crate::macros::MacroScope::new(prog.macros.clone())), &pre[0].name, Some(ctx), true).unwrap();
        let prep = crate::program::prepare_file(&mut pf, &cfg);
        prog.front_files[0].2 = crate::crate_image::file_meta(&path, &pre[0].name, &pf, &used.unwrap(), &prep, false);
        assert!(sink.file(&path, &mut pf));
        pf.sink = Some(sink.clone());
        prog.files[0] = std::sync::Arc::new(pf);
        assert!(prog.files[0].part(&cfg).unwrap().image.is_some());
        let u = Unit::new(prog, crate::tcx::Tcx::new(), 0);
        let mut c = open_test_cache(&u, &root);
        c.record_front_crates(&u.prog);
        c.flush().unwrap();
        let mut checked = 0;
        for (ci, store) in c.crate_path.iter().enumerate() {
            let st = Store::open(store).unwrap();
            for (i, path) in u.prog.file_paths.iter().enumerate().filter(|(i, _)| u.prog.file_crate[*i] as usize == ci) {
                let hash = u.prog.files[i].source_hash(path);
                for key in [Cache::front_key(path, hash), Cache::front_part_key(path, hash)] {
                    let Some((kind, b)) = st.get(key) else { continue };
                    assert_eq!(kind, KIND_FRONT_REF);
                    let (name, image) = front_run::decode_ref(b).unwrap();
                    let run = Store::open(&c.dir.join(name)).expect("the run store's committed index");
                    assert_eq!(run.get(image).map(|x| x.0), Some(KIND_FRONT), "{path}: a reference to an uncommitted image");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 4, "3 files and 1 part");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn front_record_rejects_unexplained_files_and_metadata_changes() {
        isolated_test!(front_record_rejects_unexplained_files_and_metadata_changes);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (root, mut prog, _) = front_fixture(sess, "invalid");
        let fps = crate::stable::crate_fingerprints(&prog);
        assert!(front_record(&prog, 0, |k| fps.get(k).copied()).is_some());
        prog.file_crate.push(0);
        prog.file_paths.push(prog.file_paths[0].clone());
        prog.files.push(prog.files[0].clone());
        let fps = crate::stable::crate_fingerprints(&prog);
        assert!(front_record(&prog, 0, |k| fps.get(k).copied()).is_none());
        prog.file_crate.pop();
        prog.file_paths.pop();
        prog.files.pop();
        prog.files[0] = std::sync::Arc::new(crate::parser::parse_owned(sess, b"pub fn value() -> u32 { 8 }".to_vec(), 2021).unwrap());
        let fps = crate::stable::crate_fingerprints(&prog);
        assert!(front_record(&prog, 0, |k| fps.get(k).copied()).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn front_record_missing_image_still_restores_dependents() {
        isolated_test!(front_record_missing_image_still_restores_dependents);
        let options = &crate::options::Options::default();
        let sess = &std::sync::Arc::new(crate::Session::default());
        let (root, prog, pre) = front_fixture(sess, "missing");
        let u = Unit::new(prog, crate::tcx::Tcx::new(), 0);
        let mut c = open_test_cache(&u, &root);
        c.record_front_crates(&u.prog);
        // A valid record without its arrays still certifies sources and dependencies.
        c.pending.retain(|(ci, e)| *ci != 0 || e.kind != KIND_FRONT_REF);
        c.spill_take().unwrap().write().unwrap();
        let images = FrontImages { dir: c.dir.clone(), ..Default::default() };
        let why = std::cell::Cell::new("");
        let image = images.candidate(options, &pre[0], &[], &c.crate_path[0], &why, None).unwrap();
        assert!(image.fp_only);
        assert_eq!(why.get(), "file image missing");
        let mut warm = crate::program::Program::new(sess);
        warm.record_front = true;
        warm.preload_images_bounded(pre, Some(&images), true, None);
        for (ci, c) in u.prog.crates.iter().enumerate() {
            let deps = c.deps.iter().map(|&(alias, dep)| (warm.syms.intern(u.prog.syms.str(alias)), dep)).collect();
            warm.load_crate(u.prog.syms.str(c.name), c.root_path(), c.edition, c.cfg.clone(), deps).unwrap();
            warm.crates[ci].proc_macro = c.proc_macro;
        }
        assert_eq!(warm.front_restored, vec![true; 3]);
        assert!(!warm.files[0].src.is_mapped());
        assert!(warm.files[1..].iter().all(|f| f.src.is_mapped()));
        assert_eq!(crate::stable::crate_fingerprints(&warm), c.fps);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn front_sources_have_distinct_keys_and_images() {
        isolated_test!(front_sources_have_distinct_keys_and_images);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let path = "same.rs";
        let first = b"fn value() -> u32 { 1 }";
        let second = b"fn value() -> u32 { 2 }";
        let mut a = crate::parser::parse_owned(sess, first.to_vec(), 2021).unwrap();
        let mut b = crate::parser::parse_owned(sess, second.to_vec(), 2021).unwrap();
        let ha = a.source_hash(path);
        let hb = b.source_hash(path);
        assert_eq!(ha, crate::crate_image::file_hash(path, first));
        assert_ne!(ha, hb);
        assert_ne!(Cache::front_key(path, ha), Cache::front_key(path, hb));
        assert_ne!(Cache::front_part_key(path, ha), Cache::front_part_key(path, hb));
        assert_ne!(Cache::front_key(path, ha), Cache::front_part_key(path, ha));

        fn image(pf: &crate::parser::ParsedFile) -> &'static [u8] {
            let bytes = crate::front::encode(pf);
            // Match the store's guaranteed 8-byte payload alignment.
            let words = Box::leak(vec![0u64; bytes.len().div_ceil(8)].into_boxed_slice());
            let mapped = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), bytes.len()) };
            mapped.copy_from_slice(&bytes);
            mapped
        }
        let ia = image(&a);
        let ib = image(&b);
        let images = HashMap::from([(Cache::front_key(path, ha), ia), (Cache::front_key(path, hb), ib)]);
        assert_eq!(images.len(), 2);
        // Equal source and array lengths must not let a different literal through.
        assert!(!crate::front::apply(path, &mut b, ia));
        assert!(!b.src.is_mapped());
        assert_eq!(&b.src[..], second);
        assert!(crate::front::apply(path, &mut a, images[&Cache::front_key(path, ha)]));
        assert!(crate::front::apply(path, &mut b, images[&Cache::front_key(path, hb)]));

        let mut expanded = crate::parser::parse_owned(sess, first.to_vec(), 2021).unwrap();
        expanded.src.extend_from_slice(b" expansion text");
        assert_eq!(expanded.source_hash(path), ha);
        assert!(!crate::front::apply(path, &mut expanded, ia));

    }

    /// A unit's constant-load reader reads that unit's data, whatever units the process makes
    /// after it. The proc-macro world's unit, made after the program's on a warm macOS start, took
    /// over a process-wide reader: the program's O2 builds folded its loads against the world's
    /// symbol tables, so a constant slice's length (unicode-segmentation's word_cat_table) became
    /// 0 in code the disk cache then kept -- the warm Stage panicked in its first draw.
    #[test]
    fn ro_reader_reads_its_own_unit() {
        isolated_test!(ro_reader_reads_its_own_unit);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-ro-unit-{}", std::process::id()));
        pack::ensure_private_dir(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, "fn main() {}\n").unwrap();
        let unit = || {
            let mut prog = crate::program::Program::new(sess);
            prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
            let mut tcx = crate::tcx::Tcx::new();
            tcx.collect(&prog);
            Unit::new(prog, tcx, 0)
        };
        // the program: a constant slice's (pointer, length) words, as const evaluation leaves them
        let a = unit();
        let table = a.data(&[7u8; 1085], 1, DataKind::Bytes);
        let mut words = table.to_le_bytes().to_vec();
        words.extend_from_slice(&1085u64.to_le_bytes());
        let fat = a.data(&words, 8, DataKind::Relocs(vec![(0, a.data_sym(table))]));
        let fat_sym = a.data_sym(fat);
        let reader = a.ro_reader().unwrap();
        assert!(matches!(reader.read(Sym { id: fat_sym.id, off: 8 }, 8), relax_backend::passes::RoRead::Bytes(1085)));
        // a second unit (the proc-macro world), whose same symbol ids name zeroed objects
        let b = unit();
        for _ in 0..4 {
            b.data(&[0u8; 16], 8, DataKind::Bytes);
        }
        assert!(matches!(reader.read(Sym { id: fat_sym.id, off: 8 }, 8), relax_backend::passes::RoRead::Bytes(1085)));
        assert!(matches!(a.ro_reader().unwrap().read(Sym { id: fat_sym.id, off: 8 }, 8), relax_backend::passes::RoRead::Bytes(1085)));
        drop(b);
        assert!(matches!(reader.read(Sym { id: fat_sym.id, off: 8 }, 8), relax_backend::passes::RoRead::Bytes(1085)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A constant moves to a per-domain block only through its pointer words; a copy that does
    /// move holds the value, so the cache names it as the copy of the constant's data (tag 10),
    /// never as a fresh block. unicode-segmentation's
    /// `word_cat_table` (a `&[(char, char, WordCat)]`) has integer words that, in some runs, equal
    /// an address inside the compile domain's block: the raw word scan copied it into a writable
    /// block, word_category's cache entry named that block as tag 8 (a fresh zeroed block per
    /// link), and every warm Stage launch then read the table's length as 0 in its first draw.
    #[test]
    fn rehome_follows_pointer_words_and_cached_copies_keep_their_value() {
        isolated_test!(rehome_follows_pointer_words_and_cached_copies_keep_their_value);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-rehome-{}", std::process::id()));
        pack::ensure_private_dir(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, "static S: u64 = 7;\nfn main() {}\n").unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let s_def = DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == "S").unwrap() as u32);
        let mut c = open_test_cache(&u, &root);
        let u64_ty = u.tcx.tys.u64_;
        let inside = u.syms.dom0() + 64;

        // integers (a [u64; 4] by its type) one of which equals an address inside the block
        let arr = u.tcx.tys.intern(TyKind::Array(u64_ty, u.tcx.tys.const_usize(4)));
        let mut words = Vec::new();
        for w in [1u64, inside, 1085, 3] {
            words.extend_from_slice(&w.to_le_bytes());
        }
        let ints = u.data(&words, 8, DataKind::Typed(arr));
        assert_eq!(crate::domain::rehome_const(&u, ints), ints, "an integer is not a pointer into the block");

        // a real reference to a static: copied per domain, and that copy is not stored
        let s = u.static_sym(s_def);
        let r = u.tcx.tys.intern(TyKind::Ref(false, u64_ty, crate::types::RegionId::ERASED));
        let refs = u.data(&u.syms.addr(s).to_le_bytes(), 8, DataKind::Typed(r));
        let copy = crate::domain::rehome_const(&u, refs);
        assert_ne!(copy, refs);
        let copy_sym = *u.syms.rw_syms.lock().unwrap().get(&copy).unwrap();
        assert_eq!(u.syms.const_copies.lock().unwrap().get(&copy_sym.id), Some(&refs));
        // named as the copy of `refs` (tag 10), and linked as a copy holding the value -- not as
        // a fresh, zeroed block
        let holder = u.data(&copy.to_le_bytes(), 8, DataKind::Relocs(vec![(0, copy_sym)]));
        let key = c.data_entry(&u, index(&u, holder)).unwrap();
        let entries: Vec<_> = c.pending.drain(..).map(|(_, e)| e).collect();
        let path = root.join("data.rcs");
        crate::store::append(&path, entries, true).unwrap();
        let store = std::sync::Arc::new(Store::open(&path).unwrap());
        for (i, ix) in store.index.iter().enumerate() {
            c.index.insert(ix.key, (0, i as u32));
        }
        c.stores.push(store);
        let linked = c.link_data(&mut u, key).unwrap();
        let word = unsafe { *(linked as *const u64) };
        assert!(word != 0 && word != refs && word != copy, "a new copy of the constant");
        assert_eq!(unsafe { *(word as *const u64) }, u.syms.addr(s), "the copy holds the value");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn index(u: &Unit, a: u64) -> u32 {
        match u.syms.kind(u.data_sym(a).id).unwrap() {
            SymKind::Data(i) => i,
            _ => unreachable!(),
        }
    }

    #[test]
    fn data_relocations_round_trip() {
        isolated_test!(data_relocations_round_trip);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-data-relocs-{}", std::process::id()));
        pack::ensure_private_dir(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, r#"
            extern "C" {
                #[link_name = "abs"] fn abs_a(x: i32) -> i32;
                #[link_name = "abs"] fn abs_b(x: i32) -> i32;
            }
            static VALUE: u64 = 7;
            static PAIR: [u64; 2] = [1, 2];
            fn function() {}
        "#).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (abs_a, abs_b, value, function, pair) = (def("abs_a"), def("abs_b"), def("VALUE"), def("function"), def("PAIR"));
        let mut c = open_test_cache(&u, &root);
        let ptr_ty = u.tcx.tys.intern(TyKind::Ptr(false, u.tcx.tys.intern(TyKind::Bool)));
        let typed = |u: &Unit, v: u64| u.data(&v.to_le_bytes(), 8, DataKind::Typed(ptr_ty));

        // Only reserved pointer sentinels are plain integers. A real heap allocation and
        // runtime-only storage must never be captured as process-specific bytes.
        for v in [0, 1, 8, u64::MAX, u64::MAX - 1] {
            let a = typed(&u, v);
            assert!(c.data_entry(&u, index(&u, a)).is_some());
        }
        let heap = Box::new(42u64);
        for v in [&*heap as *const u64 as u64, u.rw_data(8, 8)] {
            let a = typed(&u, v);
            assert!(c.data_entry(&u, index(&u, a)).is_none());
            assert_eq!(c.last_why, "data object: pointer words");
        }

        let child = u.data(b"child", 1, DataKind::Bytes);
        u.syms.set_data_name(child, "test:child".to_string());
        let target = Sym { id: u.data_sym(child).id, off: 2 };
        let rt = u.syms.find_rt("memcpy").unwrap();
        let a = typed(&u, u.syms.addr(rt));
        assert_eq!(crate::syms::obj_pointer_words(&u, index(&u, a)).unwrap(), vec![(0, rt)]);
        assert!(c.data_entry(&u, index(&u, a)).is_some());

        let foreign = u.foreign_sym(abs_b).unwrap();
        let a = typed(&u, u.syms.addr(foreign));
        assert_eq!(crate::syms::obj_pointer_words(&u, index(&u, a)).unwrap(), vec![(0, foreign)]);
        let foreign_key = c.data_entry(&u, index(&u, a)).unwrap();
        u.syms.foreign_defs.lock().unwrap().insert(foreign.id, abs_a);
        let a = typed(&u, u.syms.addr(foreign));
        assert_eq!(c.data_entry(&u, index(&u, a)), Some(foreign_key));
        let static_sym = u.static_sym(value);
        let id = u.fn_id(FnKey::Inst(function, Vec::new()));
        let thunk = u.syms.intern(SymKind::Thunk(id), u.entry(id));
        for sym in [static_sym, thunk] {
            let a = typed(&u, u.syms.addr(sym));
            assert_eq!(crate::syms::obj_pointer_words(&u, index(&u, a)).unwrap(), vec![(0, sym)]);
            assert!(c.data_entry(&u, index(&u, a)).is_some());
        }

        // a pointer into a static is the static plus an offset; one past its end may be the
        // next block's start and is not attributed
        let pair_sym = u.static_sym(pair);
        let at = u.syms.addr(pair_sym);
        let a = typed(&u, at + 8);
        assert_eq!(crate::syms::obj_pointer_words(&u, index(&u, a)).unwrap(), vec![(0, Sym { id: pair_sym.id, off: 8 })]);
        assert!(c.data_entry(&u, index(&u, a)).is_some());
        let a = typed(&u, at + 16);
        assert!(crate::syms::obj_pointer_words(&u, index(&u, a)).is_none());

        // a shared pointer into a promoted block an initializer's code wrote: the block's bytes
        // (pointers relocated) by its type; a `*mut` to it, and an untyped block, stay unattributed
        let block_ty = u.tcx.tys.intern(TyKind::Array(ptr_ty, u.tcx.tys.const_usize(2)));
        let block = u.rw_sym(16, 8, Some(block_ty));
        let b = u.syms.addr(block);
        unsafe {
            *(b as *mut u64) = u.syms.addr(static_sym);
            *((b + 8) as *mut u64) = 0;
        }
        let a = typed(&u, b + 8);
        let words = crate::syms::obj_pointer_words(&u, index(&u, a)).unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].1.off, 8);
        let copy = match u.syms.kind(words[0].1.id) { Some(SymKind::Data(i)) => i, k => panic!("{:?}", k) };
        assert_eq!(crate::syms::obj_pointer_words(&u, copy).unwrap(), vec![(0, static_sym)]);
        assert!(c.data_entry(&u, index(&u, a)).is_some());
        let mut_ty = u.tcx.tys.intern(TyKind::Ptr(true, u.tcx.tys.intern(TyKind::Bool)));
        let a = u.data(&(b + 8).to_le_bytes(), 8, DataKind::Typed(mut_ty));
        assert!(crate::syms::obj_pointer_words(&u, index(&u, a)).is_none());
        let untyped = u.syms.addr(u.rw_sym(16, 8, None));
        let a = typed(&u, untyped);
        assert!(crate::syms::obj_pointer_words(&u, index(&u, a)).is_none());

        // Packed offsets and an explicit symbol+addend survive linking without reverse
        // lookup. Names keep equal-content objects distinct and survive re-encoding.
        let mut bytes = vec![0x5a; 42];
        let relocs = vec![(1, target), (9, rt), (17, static_sym), (25, thunk), (33, foreign)];
        for (off, sym) in &relocs {
            bytes[*off as usize..*off as usize + 8].copy_from_slice(&u.syms.addr(*sym).to_le_bytes());
        }
        let a = u.data(&bytes, 1, DataKind::Relocs(relocs.clone()));
        u.syms.set_data_name(a, "test:parent".to_string());
        let key = c.data_entry(&u, index(&u, a)).unwrap();
        let b = u.data(&bytes, 1, DataKind::Relocs(relocs));
        u.syms.set_data_name(b, "test:other".to_string());
        assert_ne!(c.data_entry(&u, index(&u, b)).unwrap(), key);
        let entries: Vec<_> = c.pending.drain(..).map(|(_, e)| e).collect();
        let expected = entries.iter().find(|e| e.key == key).unwrap().payload.clone();
        let path = root.join("data.rcs");
        crate::store::append(&path, entries, true).unwrap();
        let store = std::sync::Arc::new(Store::open(&path).unwrap());
        for (i, ix) in store.index.iter().enumerate() {
            c.index.insert(ix.key, (0, i as u32));
        }
        c.stores.push(store);
        let linked = c.link_data(&mut u, key).unwrap();
        assert_ne!(linked, a);
        let linked_idx = index(&u, linked);
        assert_eq!(u.syms.obj_name(linked_idx).as_deref(), Some("test:parent"));
        let linked_ptrs = crate::syms::obj_pointer_words(&u, linked_idx).unwrap();
        assert_eq!(linked_ptrs[0].0, 1);
        assert_eq!(linked_ptrs[0].1.off, 2);
        assert_eq!(&linked_ptrs[1..], &[(9, rt), (17, static_sym), (25, thunk), (33, foreign)]);
        assert!(matches!(crate::syms::read_obj(&u, Sym { id: u.data_sym(linked).id, off: 1 }, 8), relax_backend::passes::RoRead::Ptr(s) if s == linked_ptrs[0].1));
        // Force recording instead of returning a memoized key / existing disk entry.
        c.index.clear();
        c.pending_keys.clear();
        assert_eq!(c.data_entry(&u, linked_idx), Some(key));
        assert_eq!(c.pending.iter().find(|(_, e)| e.key == key).unwrap().1.payload, expected);
        drop(c);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod nounwind_tests {
    use super::*;

    #[test]
    fn entries_keep_false_and_true_answers_before_code() {
        isolated_test!(entries_keep_false_and_true_answers_before_code);
        let mut e = E::default();
        e.u8(0);
        e.u32(0);
        let start = e.0.len();
        e.u32(2);
        e.bytes(b"may unwind instance"); e.u8(0);
        e.bytes(b"nounwind instance"); e.u8(1);
        let end = e.0.len();
        // the dependency certificate (none)
        e.u32(0);
        e.u128(0);
        e.u32(0);
        e.bytes(b"machine code");
        let mut d = D { b: &e.0, p: 0 };
        assert_eq!(skip_build_inputs(&mut d), Some((start, end)));
        assert_eq!(d.bytes(), Some(&b"machine code"[..]));
        for len in start..end {
            let mut d = D { b: &e.0[..len], p: 0 };
            assert!(skip_build_inputs(&mut d).is_none());
        }
    }
}

/// A link operation holds no cache borrow across calls into the Unit. The owner thread
/// serializes each cache access, while initializer evaluation may re-enter with a
/// separate Linker. The boxed cache remains installed (and at a stable address).
struct Linker {
    cache: *mut Cache,
}

impl Linker {
    /// The caller is on the owner thread and keeps the cache alive for this operation.
    unsafe fn new(cache: *mut Cache) -> Self { Self { cache } }

    /// Only borrow for an individual map/codec operation, never across Unit callbacks.
    fn cache(&mut self) -> &mut Cache { unsafe { &mut *self.cache } }

    fn dec_fnkey(&mut self, u: &mut Unit, d: &mut D, depth: u32) -> Option<FnKey> {
        if depth > 32 {
            return None;
        }
        Some(match d.u8()? {
            0 => {
                let def = self.cache().dec_def(u, d)?;
                FnKey::Inst(def, self.cache().dec_tys(u, d, 0)?)
            }
            1 => {
                let file = self.cache().dec_file(u, d)?;
                let expr = d.u32()?;
                if !self.cache().closure_fp_agrees(u, file, expr, d.u64()?) { return None; }
                // a parent the startup image names: its slot, decoded or not
                let parent = match self.image_slot_at(u, d) {
                    Some(p) => p,
                    None => {
                        let pk = self.dec_fnkey(u, d, depth + 1)?;
                        u.fn_id(pk)
                    }
                };
                FnKey::Closure(file, expr, parent)
            }
            2 => {
                let g = d.u8()?;
                FnKey::Glue(g, self.cache().dec_ty(u, d, 0)?)
            }
            3 => {
                use relax_types::instance::{ShimKind, ReifyReason};
                let tag = d.u8()?;
                let def = self.cache().dec_def(u, d)?;
                let kind = match tag {
                    0 => ShimKind::VTable(def),
                    1 => ShimKind::Reify(def, match d.u8()? { 0 => None, 1 => Some(ReifyReason::FnPtr), 2 => Some(ReifyReason::Vtable), _ => return None }),
                    2 => ShimKind::FnPtr(def, self.cache().dec_ty(u, d, 0)?),
                    3 => ShimKind::ClosureOnce { call_once: def, closure: self.cache().dec_def(u, d)?, track_caller: match d.u8()? { 0 => false, 1 => true, _ => return None } },
                    4 => ShimKind::FnPtrAsPtr(def, self.cache().dec_ty(u, d, 0)?),
                    5 => ShimKind::FnPtrFromPtr(def, self.cache().dec_ty(u, d, 0)?),
                    _ => return None,
                };
                FnKey::Shim(crate::jit::ShimInstance { kind, args: self.cache().dec_tys(u, d, 0)? })
            }
            // SPEC-V: an unverified-mode twin, keyed by its base (a base the startup image names: its slot)
            4 => {
                let base = match self.image_slot_at(u, d) {
                    Some(b) => b,
                    None => {
                        let bk = self.dec_fnkey(u, d, depth + 1)?;
                        u.fn_id(bk)
                    }
                };
                FnKey::Plain(base)
            }
            _ => return None,
        })
    }

    fn slot_of_bytes(&mut self, u: &mut Unit, b: &[u8]) -> Option<u32> {
        // a key of the startup image: its slot (image.rs)
        if let Some(s) = self.cache().image_ids.as_ref().and_then(|i| i.slot_of(b)) {
            return Some(s);
        }
        // exact bytes as the key (a fast hash, compared in full: never a wrong slot)
        if let Some(&s) = self.cache().slot_by_bytes.get(b) {
            return Some(s);
        }
        let mut d = D { b, p: 0 };
        let k = self.dec_fnkey(u, &mut d, 0)?;
        let s = u.fn_id(k);
        self.cache().slot_by_bytes.insert(b.into(), s);
        Some(s)
    }

    fn link_sym(&mut self, u: &mut Unit, d: &mut D, rw: &mut Vec<Sym>) -> Option<u64> {
        let s = self.link_symbol(u, d, rw)?;
        Some(u.syms.value(s))
    }

    /// Retain the encoded target's identity: reverse lookup by address loses aliases and
    /// cannot describe every linked symbol. Data folding must see the same target as cold code.
    /// `rw`: the writable blocks the entry's earlier symbols made, by ordinal (enc_sym).
    fn link_symbol(&mut self, u: &mut Unit, d: &mut D, rw: &mut Vec<Sym>) -> Option<Sym> {
        let tag = d.u8()?;
        let mut s = match tag {
            // runtime helpers the lowering interns on first use (ext_get / ext_free / divrem128) are interned here the
            // same way: a warm start links the entry before any lowering has asked for them
            1 => { let n = d.str()?; match u.syms.find_rt(n) { Some(s) => s, None => crate::lower::intern_lazy_rt(u, n)? } }
            2 => {
                let k = d.u128()?;
                let a = self.link_data(u, k)?;
                u.data_sym(a)
            }
            3 => {
                let def = self.cache().dec_def(u, d)?;
                if u.prog.def(def).kind != DefKind::Static {
                    return None;
                }
                let s = u.static_sym(def);
                if u.syms.value(s) == 0 || crate::jit::static_deferred(def) {
                    return None;
                }
                s
            }
            4 => {
                let def = self.cache().dec_def(u, d)?;
                u.foreign_sym(def)?
            }
            // 5 was an asm! stub of an older loader: an entry naming one is not used
            5 => return None,
            6 => {
                let file = self.cache().dec_file(u, d)?;
                let pos = d.u32()?;
                let msg = d.str()?;
                u.site_sym(file, pos, msg)
            }
            7 => {
                let id = match self.image_slot_at(u, d) {
                    Some(id) => id,
                    None => {
                        let t = crate::jit::wall_ns();
                        let key = self.dec_fnkey(u, d, 0)?;
                        let id = u.fn_id(key);
                        THUNK_DECODED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        THUNK_DECODE_NS.fetch_add(crate::jit::wall_ns() - t, std::sync::atomic::Ordering::Relaxed);
                        id
                    }
                };
                u.syms.intern(SymKind::Thunk(id), u.entry(id))
            }
            12 => {
                let id = match self.image_slot_at(u, d) {
                    Some(id) => id,
                    None => {
                        let key = self.dec_fnkey(u, d, 0)?;
                        u.fn_id(key)
                    }
                };
                crate::jit::loop_counter_sym(u, id)
            }
            8 => {
                let sz = d.u64()?;
                let al = d.u64()?;
                let ord = d.u32()? as usize;
                if ord == rw.len() {
                    rw.push(u.rw_sym(sz, al, None));
                }
                *rw.get(ord)?
            }
            11 => {
                let shape = d.u8()?;
                let def = self.cache().dec_def(u, d)?;
                if u.prog.def(def).kind != DefKind::Static {
                    return None;
                }
                let pl = u.thread_local_place(def)?;
                if tls_shape(&pl) != shape {
                    return None;
                }
                pl.sym
            }
            // the per-domain copy of constant data object `k` (domain.rs const_copy): one copy
            // per object this run, as the cold run had one per constant
            10 => {
                let k = d.u128()?;
                let a = self.link_data(u, k)?;
                let c = match self.cache().const_copy.get(&k) {
                    Some(&c) => c,
                    None => {
                        let c = crate::domain::const_copy(u, a);
                        if c == a {
                            return None;
                        }
                        self.cache().const_copy.insert(k, c);
                        c
                    }
                };
                // the code addresses it as a domain symbol, as it did the cold run's copy
                *u.syms.rw_syms.lock().unwrap_or_else(|x| x.into_inner()).get(&c)?
            }
            9 => {
                let name = d.str()?;
                let value = provided_value(u, name)?;
                u.syms.intern(SymKind::Provided(name.to_string()), value)
            }
            _ => return None,
        };
        s.off = d.u64()? as i64;
        Some(s)
    }

    fn link_data(&mut self, u: &mut Unit, key: u128) -> Option<u64> {
        if let Some(a) = self.cache().data_addr.get(&key) {
            return Some(*a);
        }
        if !self.cache().linking_data.insert(key) { return None; }
        let _link = LinkGuard { cache: self.cache, key, data: true };
        let (kind, payload) = { let c = self.cache(); let (k, p) = c.get_checked(key)?; (k, p.to_vec()) };
        if kind != KIND_DATA {
            return None;
        }
        let mut d = D { b: &payload, p: 0 };
        if d.u32()? != ENTRY_VERSION {
            return None;
        }
        let align = d.u64()?;
        let name = match d.u8()? {
            0 => None,
            1 => Some(d.str()?.to_string()),
            _ => return None,
        };
        let mut bytes = d.bytes()?.to_vec();
        let n = d.u32()? as usize;
        let mut ptrs = Vec::new();
        let mut end = 0;
        let mut rw = Vec::new();
        for _ in 0..n {
            let o = d.u32()?;
            let at = o as usize;
            if at < end || at.checked_add(8)? > bytes.len() {
                return None;
            }
            let s = self.link_symbol(u, &mut d, &mut rw)?;
            bytes[at..at + 8].copy_from_slice(&u.syms.addr(s).to_le_bytes());
            ptrs.push((o, s));
            end = at + 8;
        }
        if d.p != payload.len() {
            return None;
        }
        let a = u.data(&bytes, align, DataKind::Relocs(ptrs));
        if let Some(name) = name {
            u.syms.set_data_name(a, name);
        }
        self.cache().data_addr.insert(key, a);
        Some(a)
    }

    pub fn lookup(&mut self, u: &mut Unit, id: u32) -> Option<LinkedCode> {
        if !self.cache().linking.insert(id) {
            relax_codegen::warm::reason(id, "cache lookup already in progress");
            return None;
        }
        let _link = LinkGuard { cache: self.cache, key: id as u128, data: false };
        if self.cache().missed.contains(&id) {
            return None;
        }
        let t0 = crate::jit::wall_ns();
        let Some(key0) = self.cache().slot_key(u, id) else {
            relax_codegen::warm::reason(id, "unstorable:fn key");
            warm_count::add(warm_count::NO_KEY, 1);
            return None;
        };
        let key = key0;
        let mut r = None;
        let mut found = false;
        // the pack stays alive by its Arc: the payload is read in place
        let at = self.cache().index_at(key);
        let pack = at.map(|(p, _)| self.cache().stores[p as usize].clone());
        // checksummed before it is linked into executable memory: a corrupt entry is a miss
        // (an empty payload: a rejected body's entry, reject_verdict -- a miss)
        if let Some((KIND_CODE, payload)) = pack.as_ref().and_then(|pk| pk.get_checked_at(at.map_or(0, |a| a.1) as usize, key)).filter(|(_, p)| !p.is_empty()) {
            // a slot its code took in changed: not this run's code (compiled and recorded anew)
            // (an undecodable payload is link_entry's failure, as before)
            if self.cert_valid(u, payload) == Some(false) {
                self.cache().cert_rejected.insert(key);
                self.cache().stats.cert_rejects += 1;
                warm_count::add(warm_count::INVALIDATED, 1);
                trace_lookup(u, id, "cert:stale");
            } else {
                found = true;
                trace_lookup(u, id, "hit");
                r = self.link_entry(u, id, payload);
            }
        }
        // SPEC-V: a body with no entry of its own whose unverified-mode twin has one (an earlier
        // session ran the twin: its callers' code was lowered before their verdicts) runs the
        // twin's stored code, as borrow_twin lends a twin's code within a session: the same body,
        // built for unverified callers, sound for every caller. Verified code recorded since
        // (a caller's O2/O3 rebuild) calls the base directly; without this its first call compiled
        // the base on the program's thread, then the base's callees, one session deeper each time.
        // It tiers up from here like any linked entry (its O3 build is recorded under its own key).
        if !found && matches!(u.fns[id as usize].key, FnKey::Inst(..) | FnKey::Closure(..)) && u.fns[id as usize].build_err.is_none() {
            if let Some((tk, _, _)) = self.cache().code_key_bytes(u, &FnKey::Plain(id)) {
                let at = self.cache().index_at(tk);
                let pack = at.map(|(p, _)| self.cache().stores[p as usize].clone());
                if let Some((KIND_CODE, payload)) = pack.as_ref().and_then(|pk| pk.get_checked_at(at.map_or(0, |a| a.1) as usize, tk)).filter(|(_, p)| !p.is_empty()) {
                    if self.cert_valid(u, payload) != Some(false) {
                        if let Some(x) = self.link_entry(u, id, payload) {
                            trace_keyed(u, id, "hit:twin", Some(tk));
                            warm_count::add(warm_count::TWIN_ENTRY, 1);
                            self.cache().stats.hits += 1;
                            warm_count::add(warm_count::HIT, 1);
                            if std::mem::take(&mut self.cache().last_base) {
                                self.cache().base_linked.insert(id);
                            }
                            self.cache().linked.insert(id);
                            self.cache().linked_key.insert(id, tk);
                            self.cache().stats.link_ns += crate::jit::wall_ns() - t0;
                            return Some(x);
                        }
                    }
                }
            }
        }
        if !found {
            if relax_codegen::warm::enabled() {
                let reason = self.cache().miss_reason(u, id, key0);
                warm_count::add(if reason.starts_with("cold record exists; lookup key changed") { warm_count::KEY_CHANGED } else if reason.starts_with("cold record exists") { warm_count::ENTRY_GONE } else { warm_count::NOT_RECORDED }, 1);
                relax_codegen::warm::reason(id, &reason);
            }
            self.cache().missed.insert(id);
            self.cache().stats.misses += 1;
            if !self.cache().cert_rejected.contains(&key) {
                warm_count::add(warm_count::NO_ENTRY, 1);
            }
            trace_lookup(u, id, "miss");
            return None;
        }
        self.cache().stats.link_ns += crate::jit::wall_ns() - t0;
        match r {
            Some(x) => {
                self.cache().stats.hits += 1;
                warm_count::add(warm_count::HIT, 1);
                if std::mem::take(&mut self.cache().last_base) {
                    self.cache().base_linked.insert(id);
                }
                self.cache().linked.insert(id);
                self.cache().linked_key.insert(id, key);
                Some(x)
            }
            None => {
                self.cache().missed.insert(id);
                self.cache().stats.link_fail += 1;
                warm_count::add(warm_count::LINK_FAIL, 1);
                trace_lookup(u, id, "linkfail");
                relax_codegen::warm::reason(id, "cached entry could not decode/link");
                None
            }
        }
    }

    /// cert_valid of the code entry `key` (None: no such code entry, or it does not decode).
    pub(super) fn entry_cert_valid(&mut self, u: &mut Unit, key: u128) -> Option<bool> {
        let (p, i) = self.cache().index_at(key)?;
        let pack = self.cache().stores[p as usize].clone();
        let (kind, payload) = pack.get_checked_at(i as usize, key)?;
        if kind != KIND_CODE || payload.is_empty() { return None; }
        let t0 = relax_backend::cpu_ns();
        let r = self.cert_valid_inner(u, payload);
        self.cache().stats.cert_check_ns += relax_backend::cpu_ns() - t0;
        r
    }

    /// Whether every slot a code entry's certificate names still has the recorded key. None:
    /// the payload does not decode.
    fn cert_valid(&mut self, u: &mut Unit, payload: &[u8]) -> Option<bool> {
        let t0 = relax_backend::cpu_ns();
        let r = self.cert_valid_inner(u, payload);
        self.cache().stats.cert_check_ns += relax_backend::cpu_ns() - t0;
        r
    }

    fn cert_valid_inner(&mut self, u: &mut Unit, payload: &[u8]) -> Option<bool> {
        let mut d = D { b: payload, p: 0 };
        if d.u32()? != ENTRY_VERSION {
            return None;
        }
        d.str()?;
        let (_, nounwind_end) = skip_build_inputs(&mut d)?;
        let mut d = D { b: payload, p: nounwind_end };
        let nc = d.u32()? as usize;
        let mut idents = Vec::with_capacity(nc.min(64));
        for _ in 0..nc { idents.push(d.u128()?); }
        let fps = d.u128()?;
        if self.cache().cert_fps(&idents) == Some(fps) {
            return Some(true);
        }
        let n = d.u32()?;
        for _ in 0..n {
            let b = d.bytes()?;
            let want = d.u128()?;
            // (a slot of the crates unchanged since the manifest keeps its recorded key)
            if self.cache().identity_unchanged(b) { continue; }
            let Some(s) = self.slot_of_bytes(u, b) else { return Some(false) };
            if self.cache().slot_key(u, s) != Some(want) {
                return Some(false);
            }
        }
        Some(true)
    }

    /// Links one code entry (sets `last_callees`, `last_tier1`).
    fn link_entry(&mut self, u: &mut Unit, id: u32, payload: &[u8]) -> Option<(Vec<u8>, Vec<(u32, u64)>, Vec<RelocAt>)> {
        #[cfg(test)]
        {
            let hook = data_tests::BEFORE_LINK.with(|hook| hook.borrow_mut().take());
            if let Some(hook) = hook { hook(u); }
        }
        let mut callees = Vec::new();
        let mut thunks = Vec::new();
        let mut data = Vec::new();
        let mut d = D { b: payload, p: 0 };
        if d.u32()? != ENTRY_VERSION {
            return None;
        }
        let _name = d.str()?;
        let tier = d.u8()?;
        let is_o3 = tier == TIER_O3;
        let ni = d.u32()? as usize;
        let mut inl = Vec::with_capacity(ni);
        for _ in 0..ni {
            let b = d.bytes()?;
            inl.push(self.slot_of_bytes(u, b)?);
        }
        // the nounwind inputs and the dependency certificate (checked by cert_valid before)
        skip_nounwind_and_cert(&mut d)?;
        let code = d.bytes()?.to_vec();
        let n = d.u32()? as usize;
        let mut relocs = Vec::with_capacity(n);
        let mut rw = Vec::new();
        for _ in 0..n {
            let o = d.u32()? as usize;
            let kind = kind_of(d.u8()?)?;
            let tag = d.b.get(d.p).copied()?;
            let v = match tag {
                20 | 21 | 25 => {
                    d.u8()?;
                    let b = d.bytes()?;
                    let sid = self.slot_of_bytes(u, b)?;
                    let sid = {
                        let c = self.cache();
                        u.twin_link_target(id, sid, |b| c.has_stored_entry(u, b))
                    };
                    if tag == 21 {
                        thunks.push(sid);
                    } else {
                        callees.push(sid);
                    }
                    u.env.reloc_value(match tag { 20 => Reloc::Slot(sid), 25 => Reloc::Call(sid), _ => Reloc::Thunk(sid) })
                }
                22 => {
                    d.u8()?;
                    u.env.reloc_value(Reloc::TlsKey)
                }
                23 => {
                    d.u8()?;
                    u.env.reloc_value(Reloc::TlsSlow)
                }
                24 => {
                    d.u8()?;
                    u.env.reloc_value(Reloc::Resume)
                }
                _ => {
                    let v = self.link_sym(u, &mut d, &mut rw)?;
                    if u.syms.data_at(v).is_some() {
                        data.push(v);
                    }
                    v
                }
            };
            if o >= code.len() {
                return None;
            }
            if let Some(w) = dump_filter().filter(|_| self.cache().verbose) {
                if _name.contains(w) {
                    eprintln!("makepad-run cache: link {} at {} {:?} tag {} = {:#x}", _name, o, kind, tag, v);
                }
            }
            relocs.push(RelocAt { at: o as u32, kind, target: Reloc::Value(v) });
        }
        if let Some(w) = dump_filter().filter(|_| self.cache().verbose) {
            if _name.contains(w) {
                let hex: String = code.iter().map(|b| format!("{:02x}", b)).collect();
                eprintln!("makepad-run cache: code {} {}", _name, hex);
            }
        }
        let raw = crate::jit::PcMap::decode(d.bytes()?);
        let nf = d.u32()? as usize;
        let mut files = Vec::with_capacity(nf);
        for _ in 0..nf {
            // a file version this run has not loaded: its positions are unknown (u32::MAX),
            // never another file's (file 0 printed std's cmp.rs:1:1 for inlined code)
            files.push(self.cache().dec_file(u, &mut d).unwrap_or(u32::MAX));
        }
        let pc = raw.into_iter().map(|(o, p)| (o, (*files.get(relax_backend::rir::pos_file(p) as usize).unwrap_or(&u32::MAX) as u64) << 32 | (p & 0xffff_ffff) | (p & !relax_backend::rir::pos_plain(u64::MAX)))).collect();
        self.cache().last_callees = callees;
        self.cache().last_thunks = thunks;
        self.cache().last_data = data;
        if !is_o3 {
            // the slots this code took in run inside it: a replacement that calls one instead
            // (an O3 build missing its body) has it built before its swap (jit prebuild_callees);
            // O3 entries register theirs at install (cached_tier1)
            for &c in &inl {
                if c != id && (c as usize) < u.fns.len() && !u.fns[c as usize].history().inlined_into.contains(&id) {
                    crate::zone_alloc::kept_with(|| u.fns[c as usize].history.get_or_insert_with(Default::default).inlined_into.push(id));
                }
            }
        }
        self.cache().last_tier1 = if is_o3 { Some(inl) } else { None };
        self.cache().last_o2 = tier == TIER_O2;
        self.cache().last_base = tier == TIER_BASE;
        Some((code, pc, relocs))
    }
}

/// Re-entrant cache lookup; the caller is on the owner thread.
pub(crate) unsafe fn lookup_unit(up: *mut Unit, id: u32) -> Option<LinkedCode> {
    let Some(cache) = (&*up).cache.as_ref().map(|c| c.get()) else { return None };
    Linker::new(cache).lookup(&mut *up, id)
}

struct LinkGuard { cache: *mut Cache, key: u128, data: bool }
impl Drop for LinkGuard {
    fn drop(&mut self) {
        // Same owner thread call and cache lifetime as the owning Linker, including unwinding.
        let c = unsafe { &mut *self.cache };
        if self.data { c.linking_data.remove(&self.key); }
        else { c.linking.remove(&(self.key as u32)); }
    }
}

/// Whether a cache entry is being linked (on the owner thread: by this thread).
pub(crate) fn linking(u: &Unit) -> bool {
    u.cache.as_ref().is_some_and(|c| { let c = unsafe { &*c.get() }; !c.linking.is_empty() || !c.linking_data.is_empty() })
}

pub(crate) unsafe fn slot_for_bytes_unit(up: *mut Unit, bytes: &[u8]) -> Option<u32> {
    let cache = (&*up).cache.as_ref()?.get();
    Linker::new(cache).slot_of_bytes(&mut *up, bytes)
}

impl Cache {
    /// The value entry of a constant (including generated array lengths) or static: keyed by its
    /// identity alone (one entry per value; a newer one replaces it), certified by what the value
    /// was computed from (`ConstCert`, checked by `const_entry`). Also the crate whose store the
    /// entry goes to and the crates the identity names.
    fn const_key(&mut self, u: &Unit, def: DefId, args: &[TyId]) -> Option<(u128, u32, Vec<u32>)> {
        // Fingerprints describe startup inputs. Live edits can change a value's
        // transitive initializer dependencies before the next patch is installed.
        if !crate::reload::edited_crates().is_empty() { return None; }
        let mut e = E::default();
        let mut crates = Vec::new();
        if self.enc_fnkey(u, &FnKey::Inst(def, args.to_vec()), &mut e, &mut crates, 0).is_none() { return None; }
        if crates.iter().any(|&c| c as usize >= self.crate_ident.len()) { return None; }
        crates.sort_unstable();
        crates.dedup();
        let mut h = Sip128::new();
        h.str("const-value-v2");
        h.u128(self.cfg);
        h.write(&e.0);
        let dom = self.dominant(&crates);
        Some((h.finish128(), dom, crates))
    }

    /// The code key, crates and identity bytes of body `key` (memo for the run)
    fn const_body_key(&mut self, u: &Unit, key: &FnKey) -> Option<(u128, Vec<u32>, Vec<u8>)> {
        if let Some(k) = self.const_body_keys.get(key) {
            return k.clone();
        }
        let outer = std::mem::replace(&mut self.key_waits, false);
        let k = self.code_key_bytes(u, key);
        if !self.key_waits { self.const_body_keys.insert(key.clone(), k.clone()); }
        self.key_waits |= outer;
        k
    }

    /// A value entry's certificate: complete (u8), the crates of its fast path (their idents and
    /// a hash of their whole fingerprints now: while it is the same, nothing the bodies are made
    /// of changed), and for a complete one each body by its identity with its code key. An
    /// incomplete certificate (the value ran code, or read one that did) holds only by the fast
    /// path over the crates its identity names (whose fingerprints cover their dependencies).
    fn const_cert_bytes(&mut self, u: &Unit, own: &[u32], cert: &crate::jit::ConstCert) -> Option<Vec<u8>> {
        let mut complete = cert.complete;
        let mut items: Vec<(Vec<u8>, u128)> = Vec::new();
        let mut crates: Vec<u32> = own.to_vec();
        let mut idents: Vec<u128> = Vec::new();
        if complete {
            for k in &cert.keys {
                match self.const_body_key(u, k) {
                    Some((key, cr, bytes)) => {
                        crates.extend(cr);
                        items.push((bytes, key));
                    }
                    None => { complete = false; break; }
                }
            }
        }
        if complete {
            items.extend(cert.items.iter().cloned());
            idents.extend(cert.idents.iter().copied());
            items.sort_unstable();
            items.dedup();
        } else {
            items.clear();
        }
        for &c in &crates { idents.push(*self.crate_ident.get(c as usize)?); }
        idents.sort_unstable();
        idents.dedup();
        let mut e = E::default();
        e.u8(complete as u8);
        e.u32(idents.len() as u32);
        for &i in &idents { e.u128(i); }
        e.u128(self.cert_fps(&idents)?);
        e.u32(items.len() as u32);
        for (b, k) in items {
            e.bytes(&b);
            e.u128(k);
        }
        Some(e.0)
    }

    pub fn record_const(&mut self, u: &Unit, def: DefId, args: &[TyId], addr: u64, cert: &crate::jit::ConstCert) -> Option<()> {
        if self.mode == Mode::ReadOnly { return None; }
        let (key, krate, own) = self.const_key(u, def, args)?;
        if self.pending_keys.contains(&key) || (self.has_index(&key) && !self.const_stale.contains(&key)) { return Some(()); }
        let SymKind::Data(idx) = u.syms.kind(u.data_sym(addr).id)? else { return None; };
        // A recursive compile can inspect this object before its initializer finishes.
        self.data_key.remove(&idx);
        self.data_why.remove(&idx);
        let previous = self.cur_crate;
        self.cur_crate = krate;
        let data = self.data_entry(u, idx);
        self.cur_crate = previous;
        let data = data?;
        let mut payload = self.const_cert_bytes(u, &own, cert)?;
        payload.extend_from_slice(&data.to_le_bytes());
        self.pending_keys.insert(key);
        self.const_live.insert(key);
        self.push_pending(krate, NewEntry { key, kind: KIND_CONST, payload });
        Some(())
    }

    /// Value entry `payload`'s data key, when its certificate holds (fast path, or every body it
    /// names has the code key it had), with the certificate a reader takes in. None: it does
    /// not hold or does not decode.
    fn const_entry(&mut self, u: &mut Unit, payload: &[u8]) -> Option<(u128, crate::jit::ConstCert)> {
        let mut d = D { b: payload, p: 0 };
        let complete = d.u8()? == 1;
        let nc = d.u32()? as usize;
        let mut idents = Vec::with_capacity(nc.min(64));
        for _ in 0..nc { idents.push(d.u128()?); }
        let fps = d.u128()?;
        let fast = self.cert_fps(&idents) == Some(fps);
        if !fast && !complete { return None; }
        let n = d.u32()? as usize;
        let mut items = Vec::with_capacity(n.min(256));
        for _ in 0..n {
            let b = d.bytes()?;
            let want = d.u128()?;
            if !fast && !self.identity_unchanged(b) {
                let key = unsafe { Linker::new(self) }.dec_fnkey(u, &mut D { b, p: 0 }, 0)?;
                if self.const_body_key(u, &key).map(|k| k.0) != Some(want) { return None; }
            }
            items.push((b.to_vec(), want));
        }
        let data = u128::from_le_bytes(d.b.get(d.p..d.p + 16)?.try_into().ok()?);
        (d.p + 16 == payload.len()).then(|| (data, crate::jit::ConstCert { keys: Vec::new(), items, idents, complete }))
    }
}

/// Hydrate a completed immutable value, including relocations, with the cache
/// available to any initializer needed to resolve its symbols. The evaluation reading it (if
/// any) takes in its certificate; this run keeps it for later readers.
pub(crate) unsafe fn const_value(up: *mut Unit, def: DefId, args: &[TyId]) -> Option<u64> {
    let c = (&*up).cache.as_ref()?.get();
    let (key, _, _) = (&mut *c).const_key(&*up, def, args)?;
    let payload = {
        let (kind, payload) = (&*c).get(key)?;
        if kind != KIND_CONST { return None; }
        // (the store's mapping outlives this lookup: the payload is read before any write)
        std::slice::from_raw_parts(payload.as_ptr(), payload.len())
    };
    let Some((data, cert)) = (&mut *c).const_entry(&mut *up, payload) else {
        (&mut *c).const_stale.insert(key);
        return None;
    };
    let a = Linker::new(c).link_data(&mut *up, data)?;
    (&mut *c).const_live.insert(key);
    crate::jit::cert_take_in(&cert);
    let u = &mut *up;
    crate::zone_alloc::kept_with(|| u.const_certs.insert((def, args.to_vec()), cert));
    Some(a)
}

impl Cache {
    fn outcome_key(&self, u: &Unit, id: u32) -> u128 {
        let mut h = Sip128::new();
        h.str("warm-outcome");
        h.u128(self.cfg);
        h.str(&crate::syms::stable_fn_name(u, id));
        h.finish128()
    }

    /// Optional diagnostic ledger: retain cold evidence for the strict run's
    /// taxonomy. An ordinary cache without this evidence reports uncertainty.
    fn remember_outcome(&mut self, u: &Unit, id: u32, code_key: Option<u128>, reason: &str) {
        if !relax_codegen::warm::enabled() || self.mode == Mode::ReadOnly { return; }
        let key = self.outcome_key(u, id);
        if !self.pending_keys.insert(key) { return; }
        let krate = match u.fns[id as usize].key {
            FnKey::Inst(d, _) => u.prog.def(d).krate,
            _ => self.cur_crate,
        };
        let mut e = E::default();
        e.u128(code_key.unwrap_or(0));
        e.str(reason);
        self.push_pending(krate, NewEntry { key, kind: KIND_WARM_OUTCOME, payload: e.0 });
    }

    fn miss_reason(&self, u: &Unit, id: u32, code_key: u128) -> String {
        let outcome = self.get(self.outcome_key(u, id)).and_then(|(kind, bytes)| {
            if kind != KIND_WARM_OUTCOME { return None; }
            let mut d = D { b: bytes, p: 0 };
            Some((d.u128()?, d.str()?.to_owned()))
        });
        match outcome {
            Some((old, reason)) if reason == "recorded" && old != code_key => "cold record exists; lookup key changed".to_owned(),
            Some((_, reason)) if reason == "recorded" => "cold record exists; code entry unavailable".to_owned(),
            Some((_, reason)) => reason,
            None => "no standalone cold lookup/record observed; precise cause unresolved".to_owned(),
        }
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    /// `makepad clean -p`: the app crate's stores and its program's graph stores (every
    /// generation); the shared crates' stores and another app's graph stay.
    #[test]
    fn app_store_files_are_the_apps_own() {
        isolated_test!(app_store_files_are_the_apps_own);
        let dir = std::env::temp_dir().join(format!("relax-appfiles-{}-{}", std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&dir).unwrap();
        let put = |name: &str, kind: u32, payload: Vec<u8>| -> PathBuf {
            let p = dir.join(name);
            crate::store::append(&p, vec![NewEntry { key: 7, kind, payload }], false).unwrap();
            p
        };
        let graph = |identity: u128, app: &str| crate::crate_image::encode(&crate::crate_image::GraphImage {
            identity, crates: vec![(("core".into(), "/s/core/lib.rs".into()), 1), ((app.into(), format!("/w/{app}/main.rs")), 2)], inputs: Vec::new(), stamps: Vec::new(), probes: Vec::new(), demand_image: None,
        });
        let own = put("calc_app-0000000000000001.rcs", KIND_DATA, vec![1]);
        let own_gen = crate::store::gen_path(&own, 1);
        std::fs::copy(&own, &own_gen).unwrap();
        let shared = put("core-0000000000000002.rcs", KIND_DATA, vec![2]);
        let longer = put("calc_app_extra-0000000000000003.rcs", KIND_DATA, vec![3]);
        let g_own = put("front_graph-0000000000000004.rcs", KIND_FRONT_GRAPH, graph(11, "calc_app"));
        let g_other = put("front_graph-0000000000000005.rcs", KIND_FRONT_GRAPH, graph(12, "other_app"));
        let c_own = collected::path(&dir, 11);
        std::fs::copy(&g_own, &c_own).unwrap();
        let c_other = collected::path(&dir, 12);
        std::fs::copy(&g_other, &c_other).unwrap();
        let mut want = vec![own.clone(), own_gen.clone(), g_own.clone(), c_own.clone()];
        want.sort();
        assert_eq!(app_store_files(&dir, "calc_app"), want);
        for p in [own, own_gen, shared, longer, g_own, g_other, c_own, c_other] {
            std::fs::remove_file(p).unwrap();
        }
        // the folder also holds this process's live lock (store::hold_build)
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_runs_stores_survive_a_tiny_cap_and_stale_ones_go_first() {
        isolated_test!(a_runs_stores_survive_a_tiny_cap_and_stale_ones_go_first);
        let root = std::env::temp_dir().join(format!("relax-prune-{}-{}", std::process::id(), crate::clock::wall_ns()));
        let dir = root.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let day = 24 * 3600;
        let store = |name: &str, age_s: u64| -> PathBuf {
            let p = dir.join(name);
            crate::store::append(&p, vec![NewEntry { key: 1, kind: KIND_DATA, payload: vec![7; 4096] }], true).unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_s);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            p
        };
        // this run's stores carry the oldest stamps, and still are never evicted
        let used = [store("bitflags-0000000000000001.rcs", 3 * day), store("memchr-0000000000000002.rcs", 3 * day)];
        let stale_old = store("bitflags-00000000000000aa.rcs", 2 * day);
        let stale_new = store("memchr-00000000000000bb.rcs", day);
        // stamped a minute ago by another run
        let other_run = store("amp-00000000000000cc.rcs", 60);
        let live: HashSet<PathBuf> = used.iter().cloned().collect();
        let all: u64 = [&used[0], &used[1], &stale_old, &stale_new, &other_run].iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
        // one store over the cap: the least recently used one not in use goes
        let (before, after) = prune(&root, &live, &HashSet::new(), &[], Some(all - 1));
        assert_eq!(before, all);
        assert!(!stale_old.exists());
        assert!(stale_new.exists());
        assert!(after < all);
        // a tiny cap: every unused store goes; the run's own and the other run's stay (reported)
        let (_, after) = prune(&root, &live, &HashSet::new(), &[], Some(1));
        assert!(!stale_new.exists());
        assert!(used.iter().all(|p| p.exists()));
        assert!(other_run.exists());
        assert!(after > 1);
        // the default cap (floor, or twice the run's stores) keeps what is left
        assert_eq!(prune(&root, &live, &HashSet::new(), &[], None).1, after);
        // a rewritten store's older generation goes at the next prune (also a store this run
        // uses), unless this run itself reads that generation file
        let stale_gen = store("serde-00000000000000dd.rcs", 60);
        std::fs::copy(&stale_gen, crate::store::gen_path(&stale_gen, 1)).unwrap();
        let used_gen = crate::store::gen_path(&used[0], 1);
        std::fs::copy(&used[0], &used_gen).unwrap();
        let used_gen1 = crate::store::gen_path(&used[1], 1);
        std::fs::copy(&used[1], &used_gen1).unwrap();
        let open: HashSet<PathBuf> = [used[0].clone()].into_iter().collect();
        prune(&root, &live, &open, &[], None);
        assert!(!stale_gen.exists() && crate::store::gen_path(&stale_gen, 1).exists());
        assert!(used[0].exists() && used_gen.exists());
        assert!(!used[1].exists() && used_gen1.exists());
        for p in [crate::store::gen_path(&stale_gen, 1), used_gen, used_gen1, used[0].clone(), other_run] {
            std::fs::remove_file(p).unwrap();
        }
        // the folder also holds this process's live lock (store::hold_build)
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }

    /// Store sets: a program's run names its store per role; a crate store the previous run's set
    /// named that the program's set no longer names (an edit changed the crate's fingerprint, a
    /// crate left the program) goes unless another program's set or a graph's demand parts name it
    /// -- once no run stamped it recently, also at a later prune (the retiring list); a run store
    /// no store refers to goes; a store no set ever named stays (the cap's business).
    #[test]
    fn replaced_crate_stores_retire_by_store_sets() {
        isolated_test!(replaced_crate_stores_retire_by_store_sets);
        let root = std::env::temp_dir().join(format!("relax-sets-{}-{}", std::process::id(), crate::clock::wall_ns()));
        let dir = root.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let stamp = |p: &PathBuf, t: std::time::SystemTime| std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
        let store = |name: &str, entries: Vec<NewEntry>| -> PathBuf {
            let p = dir.join(name);
            crate::store::append(&p, entries, true).unwrap();
            stamp(&p, old);
            p
        };
        let data = || vec![NewEntry { key: 1, kind: KIND_DATA, payload: vec![7; 64] }];
        let reference = |run: &str| vec![NewEntry { key: 2, kind: KIND_FRONT_REF, payload: front_run::encode_ref(&crate::parser::ImageLoc { store: run.into(), key: 9 }) }];
        let run_kept = store("front_run-00000000000000a1.rcs", data());
        let run_dead = store("front_run-00000000000000a2.rcs", data());
        let code_v1 = store("widgets-0000000000000001.rcs", data());
        let code_v2 = store("widgets-0000000000000011.rcs", data());
        let front_v1 = store("widgets-0000000000000021.rcs", reference("front_run-00000000000000a1.rcs"));
        let front_v2 = store("widgets-0000000000000031.rcs", data());
        let removed = store("removed-0000000000000004.rcs", data());
        let shared_v1 = store("shared-0000000000000005.rcs", data());
        let shared_v2 = store("shared-0000000000000015.rcs", data());
        let unnamed = store("unnamed-0000000000000006.rcs", data());
        // a retained demand part still takes images from the old front store of `parted`
        let parted_v1 = store("parted-0000000000000007.rcs", data());
        let parted_v2 = store("parted-0000000000000017.rcs", data());
        let graph = store("front_graph-0000000000000008.rcs", vec![NewEntry { key: 3, kind: KIND_DEMAND_STORES, payload: b"parted-0000000000000007.rcs\n".to_vec() }]);
        let me = crate::store::this_build();
        let (prog, other) = (0x5eed_u64, 0x0ade_u64);
        let sets = dir.join("sets");
        std::fs::create_dir_all(&sets).unwrap();
        std::fs::write(sets.join(format!("{me:016x}-{prog:016x}")), "code w widgets-0000000000000001.rcs\nfront w widgets-0000000000000021.rcs\ncode r removed-0000000000000004.rcs\ncode s shared-0000000000000005.rcs\nfront p parted-0000000000000007.rcs\ngraph front_graph-0000000000000008.rcs\n").unwrap();
        // another program of this build still uses the old `shared`
        std::fs::write(sets.join(format!("{me:016x}-{other:016x}")), "code s shared-0000000000000005.rcs\n").unwrap();
        let roles: std::collections::BTreeMap<String, String> = [("code w", "widgets-0000000000000011.rcs"), ("front w", "widgets-0000000000000031.rcs"),
            ("code s", "shared-0000000000000015.rcs"), ("front p", "parted-0000000000000017.rcs"), ("graph", "front_graph-0000000000000008.rcs")].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let live: HashSet<PathBuf> = [code_v2.clone(), front_v2.clone(), parted_v2.clone(), graph.clone()].into_iter().collect();
        // a replaced store another run stamped recently stays for now
        stamp(&removed, std::time::SystemTime::now());
        prune(&root, &live, &HashSet::new(), &[(prog, roles.clone())], Some(u64::MAX));
        assert!(!code_v1.exists() && !front_v1.exists(), "replaced crate stores go");
        assert!(parted_v1.exists(), "a demand part takes images from it");
        assert!(removed.exists(), "stamped recently");
        assert!(shared_v1.exists(), "another program's set names it");
        assert!(code_v2.exists() && front_v2.exists() && shared_v2.exists());
        assert!(unnamed.exists(), "never named: the cap's business");
        assert!(!run_kept.exists() && !run_dead.exists(), "run stores: go once no store refers to them (front_v1 went)");
        let set = std::fs::read_to_string(sets.join(format!("{me:016x}-{prog:016x}"))).unwrap();
        assert_eq!(set, "code s shared-0000000000000015.rcs\ncode w widgets-0000000000000011.rcs\nfront p parted-0000000000000017.rcs\nfront w widgets-0000000000000031.rcs\ngraph front_graph-0000000000000008.rcs\n");
        assert_eq!(std::fs::read_to_string(sets.join(format!("{me:016x}.retiring"))).unwrap(), "parted-0000000000000007.rcs\nremoved-0000000000000004.rcs\n");
        // later: the retiring store is no longer recent and goes; the other program dropping `shared` too lets it go
        stamp(&removed, old);
        std::fs::write(sets.join(format!("{me:016x}-{other:016x}")), "code s shared-0000000000000015.rcs\n").unwrap();
        let roles2 = roles.clone();
        prune(&root, &live, &HashSet::new(), &[(prog, roles)], Some(u64::MAX));
        assert!(!removed.exists());
        assert!(shared_v1.exists(), "dropped by the other program without a run of it here: not on the list");
        // the part was encoded again against the new store: the old one goes
        crate::store::append(&graph, vec![NewEntry { key: 3, kind: KIND_DEMAND_STORES, payload: b"parted-0000000000000017.rcs\n".to_vec() }], true).unwrap();
        stamp(&graph, old);
        prune(&root, &live, &HashSet::new(), &[(prog, roles2)], Some(u64::MAX));
        assert!(!parted_v1.exists());
        assert_eq!(std::fs::read_to_string(sets.join(format!("{me:016x}.retiring"))).unwrap(), "");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Another build's stores go at the next prune (no build but their own ever reads them),
    /// stamped recently or not and under any cap, unless a process of that build is running:
    /// it holds that build's live lock (store::hold_build). This build's stores keep the cap's
    /// LRU rule.
    #[test]
    fn other_builds_stores_retire_unless_that_build_runs() {
        isolated_test!(other_builds_stores_retire_unless_that_build_runs);
        let root = std::env::temp_dir().join(format!("relax-retire-{}-{}", std::process::id(), crate::clock::wall_ns()));
        let dir = root.join("cache");
        std::fs::create_dir_all(&dir).unwrap();
        let store = |name: &str, age_s: u64, build: Option<u64>| -> PathBuf {
            let p = dir.join(name);
            crate::store::append(&p, vec![NewEntry { key: 1, kind: KIND_DATA, payload: vec![7; 4096] }], true).unwrap();
            if let Some(b) = build {
                crate::store::set_build_for_test(&p, b);
            }
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(age_s);
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            p
        };
        let me = crate::store::this_build();
        let (gone, alive) = (me ^ 0x1111, me ^ 0x2222);
        let mine_old = store("core-0000000000000001.rcs", 3600, None);
        let mine_used = store("alloc-0000000000000002.rcs", 3600, None);
        // stamped a minute ago: the old mtime window would have kept it
        let other_recent = store("core-00000000000000a1.rcs", 60, Some(gone));
        let other_gen = crate::store::gen_path(&other_recent, 1);
        std::fs::copy(&other_recent, &other_gen).unwrap();
        // written before headers named their build
        let unnamed = store("std-00000000000000a2.rcs", 3600, Some(0));
        let other_running = store("core-00000000000000b1.rcs", 3600, Some(alive));
        assert_eq!(crate::store::build_of(&mine_old), Some(me));
        assert_eq!(crate::store::build_of(&other_running), Some(alive));
        // a process of build `alive` (its lock, as hold_build takes it)
        std::fs::create_dir_all(dir.join("live")).unwrap();
        let lock = std::fs::File::create(dir.join("live").join(format!("{alive:016x}"))).unwrap();
        lock.lock_shared().unwrap();
        // an ended build's lock file with no stores left
        let ended = dir.join("live").join(format!("{:016x}", me ^ 0x3333));
        std::fs::File::create(&ended).unwrap();
        let live: HashSet<PathBuf> = [mine_used.clone()].into_iter().collect();
        prune(&root, &live, &HashSet::new(), &[], Some(u64::MAX));
        assert!(!other_recent.exists() && !other_gen.exists() && !unnamed.exists());
        assert!(other_running.exists(), "a running build's stores stay");
        assert!(mine_old.exists() && mine_used.exists(), "this build's stores are the cap's business");
        assert!(dir.join("live").join(format!("{me:016x}")).exists(), "this process holds its build's lock");
        #[cfg(unix)]
        assert!(!ended.exists(), "an ended build's lock file goes");
        // that build's process ends: its stores go at the next prune
        drop(lock);
        prune(&root, &live, &HashSet::new(), &[], Some(u64::MAX));
        assert!(!other_running.exists());
        #[cfg(unix)]
        assert!(!dir.join("live").join(format!("{alive:016x}")).exists());
        // this build's stores: unchanged LRU under the cap (the run's own stay)
        prune(&root, &live, &HashSet::new(), &[], Some(1));
        assert!(!mine_old.exists() && mine_used.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// (trace) thunk symbols whose fn key was not an image key: decoded, and the time
pub(crate) static THUNK_DECODED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static THUNK_DECODE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Entries recorded so far, in every store (a reader that saw no new one need not ask the owner
/// for `pending_bytes` again: run.rs spill_cache).
pub static RECORDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

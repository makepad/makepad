//! The digest gate (lane JITPERF; the re-key verdict of 11-17). A patch builds every slot some
//! walk selects again: the edited bodies, a changed const's readers, re-keyed slots, retargets
//! and rebound hosts. Many of those do not change. The gate compares each selected slot's new
//! lowering with the lowering its running code was built from, and drops the equal ones. A host
//! whose own body is equal but which copied a changed body is still rebuilt, so new calls run new
//! code.
//!
//! This first step only counts. Every code version keeps its digests: the code index for code
//! built or linked this run, a cache entry's tail, a startup image slot's tail. A patch's installs
//! are classed as the gate would class them. With RELAX_RELOAD_TRACE each patch prints
//! `gate: N selected, D digest-equal, H hosts, C changed, U unknown` and what the digests cost;
//! RELAX_GATE_TRACE names each slot's class. Nothing is decided by it yet.
//!
//! A digest (`rir::BodyDigest`) is taken at lowering, before inlining and passes. It numbers the
//! slots and symbols it names, and a resolver turns those into identities: a slot's instance
//! identity (the cache's key identity bytes), a data object's content, a static by its definition,
//! a panic site by its file path, line, column and message. A record holds the resolved digest.
//! A run without a cache resolves by this run's ids, which only this run compares. Unknown
//! (no digest, an identity that cannot be named) counts as changed: never less safe than today.

use crate::jit::Unit;
use crate::syms::SymKind;
use relax_backend::rir::{BodyDigest, Func};
use relax_session::sip::Sip128;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A digest as code keeps it.
#[derive(Clone, Debug)]
pub(crate) enum Dg {
    /// taken this run: resolved when compared or recorded
    Taken(Arc<BodyDigest>),
    /// read from a record (a cache entry, a startup image slot)
    Resolved(u128),
    Unknown,
}

/// The install path that supplied a code version's digests (or had none to supply).
#[derive(Clone, Copy, Debug)]
pub(crate) enum DigestOrigin {
    StartupImage,
    CacheLink,
    Built,
    Linked,
    O3First,
    O3,
    PatchBuild,
}

impl DigestOrigin {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::StartupImage => "startup image",
            Self::CacheLink => "cache link",
            Self::Built => "built",
            Self::Linked => "linked",
            Self::O3First => "O3 first",
            Self::O3 => "O3",
            Self::PatchBuild => "patch build",
        }
    }
}

/// What a code version was built from: its own lowering's digest and, per inline context, the
/// context's slot (NO_SLOT: unknown), the digest of the body it took in, and whether a frame can
/// wait inside that copy (it holds a call or a cycle).
#[derive(Debug)]
pub(crate) struct CodeDigest {
    pub(crate) own: Dg,
    pub(crate) ctx: Box<[(u32, Dg, bool)]>,
}

pub(crate) const NO_SLOT: u32 = u32::MAX;

/// cpu ns and bodies of the lowering digests taken this run (their cost: a cold start pays it on
/// every body)
static DIGEST_NS: AtomicU64 = AtomicU64::new(0);
static DIGEST_N: AtomicU64 = AtomicU64::new(0);
/// cpu ns of lowering while a patch is open
static PATCH_LOWER_NS: AtomicU64 = AtomicU64::new(0);
/// Closure ordinals whose owner has no item in the file version the closure names.
pub(crate) static CLOSURE_ORDINAL_FILE_MISSES: AtomicU64 = AtomicU64::new(0);

/// What this module holds of a run, as a fresh process has it (`run_end`).
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&DIGEST_NS, AtomicU64::new(0));
    reset(&DIGEST_N, AtomicU64::new(0));
    reset(&PATCH_LOWER_NS, AtomicU64::new(0));
    reset(&CLOSURE_ORDINAL_FILE_MISSES, AtomicU64::new(0));
    reset(&PATCH, relax_session::sync::Mutex::new(None));
    reset(&UNNOTED, relax_session::sync::Mutex::new(None));
}

/// The digest of `f` as lowered (`Func::lowering_digest`), kept off the compile's scratch heap.
pub(crate) fn take(f: &Func) -> Arc<BodyDigest> {
    let t = relax_session::clock::cpu_ns();
    let d = crate::zone_alloc::kept_with(|| Arc::new(f.lowering_digest()));
    DIGEST_NS.fetch_add(relax_session::clock::cpu_ns().saturating_sub(t), Ordering::Relaxed);
    DIGEST_N.fetch_add(1, Ordering::Relaxed);
    d
}

/// Lowering time while a patch is open (the gate's cost is a lowering per selected slot).
pub(crate) fn note_lowering(ns: u64) {
    if crate::jit::PATCH_OPEN.load(Ordering::Relaxed) {
        PATCH_LOWER_NS.fetch_add(ns, Ordering::Relaxed);
    }
}

/// Whether the code index keeps each code version's digests: a live session, where a patch
/// compares them (records take theirs from the build either way). A run without edits keeps none.
pub(crate) fn keeping() -> bool {
    crate::layout::LIVE_SESSION.load(Ordering::Relaxed)
}

/// The digests of code built from `f`.
pub(crate) fn of_func(f: &Func) -> Arc<CodeDigest> {
    crate::zone_alloc::kept_with(|| {
        let ctx = (0..f.inl_slots.len())
            .map(|k| (f.inl_slots[k], f.inl_digest.get(k).cloned().flatten().map_or(Dg::Unknown, Dg::Taken), f.inl_waits.get(k).copied().unwrap_or(true)))
            .collect();
        Arc::new(CodeDigest { own: f.digest.clone().map_or(Dg::Unknown, Dg::Taken), ctx })
    })
}

/// The digests of the code slot `id` runs now (None: unknown).
pub(crate) fn running(u: &Unit, id: u32) -> Option<Arc<CodeDigest>> {
    if id as usize >= u.fns.len() {
        return None;
    }
    let at = u.fns[id as usize].code;
    if at == 0 {
        return None;
    }
    u.code.read().unwrap_or_else(|e| e.into_inner()).digest_at(at)
}

// ------------------------------------------------------------ identities

/// Where slot and definition identities come from: the cache's key identity bytes (they hold
/// across runs), or this run's ids.
pub(crate) trait Ids {
    fn slot(&mut self, u: &Unit, x: u32) -> Option<u128>;
    fn def(&mut self, u: &Unit, d: crate::program::DefId) -> Option<u128>;
    /// (probes only) where slot `x`'s identity bytes come from now, and the bytes
    fn slot_bytes_from(&mut self, _u: &Unit, _x: u32) -> (&'static str, Option<Vec<u8>>) {
        ("this run's id", None)
    }
}

/// This run's ids (no cache: nothing is recorded, only this run compares).
pub(crate) struct Session;

impl Ids for Session {
    fn slot(&mut self, _: &Unit, x: u32) -> Option<u128> {
        Some(x as u128)
    }
    fn def(&mut self, _: &Unit, d: crate::program::DefId) -> Option<u128> {
        Some(d.0 as u128)
    }
}

/// Resolves taken digests to identities (memoised for its lifetime: the digests it is given stay
/// alive while it runs).
pub(crate) struct Resolver<'a, I: Ids + ?Sized> {
    u: &'a Unit,
    ids: &'a mut I,
    slots: HashMap<u32, Option<u128>>,
    syms: HashMap<u32, Option<u128>>,
    taken: HashMap<usize, Option<u128>>,
}

impl<'a, I: Ids + ?Sized> Resolver<'a, I> {
    pub(crate) fn new(u: &'a Unit, ids: &'a mut I) -> Self {
        Resolver { u, ids, slots: HashMap::new(), syms: HashMap::new(), taken: HashMap::new() }
    }

    pub(crate) fn resolve(&mut self, d: &Dg) -> Option<u128> {
        match d {
            Dg::Resolved(x) => Some(*x),
            Dg::Unknown => None,
            Dg::Taken(b) => {
                let k = Arc::as_ptr(b) as usize;
                if let Some(r) = self.taken.get(&k) {
                    return *r;
                }
                let r = (|| {
                    let mut h = Sip128::new();
                    h.str("relax gate body");
                    h.u128(b.raw);
                    h.u32(b.slots.len() as u32);
                    for &s in &b.slots {
                        h.u128(self.slot(s)?);
                    }
                    h.u32(b.syms.len() as u32);
                    for &y in &b.syms {
                        h.u128(self.sym(y)?);
                    }
                    Some(h.finish128())
                })();
                self.taken.insert(k, r);
                r
            }
        }
    }

    /// Every name a taken digest holds, slots first, then symbols, with its identity (a probe's
    /// listing: which name's identity moved). A slot's bytes and their source are read before its
    /// identity, so a source is the one this resolution used.
    pub(crate) fn listing(&mut self, d: &BodyDigest) -> Vec<Named> {
        let mut v = Vec::with_capacity(d.slots.len() + d.syms.len());
        for &s in &d.slots {
            let (from, bytes) = if self.slots.contains_key(&s) { ("resolved before", None) } else { self.ids.slot_bytes_from(self.u, s) };
            v.push(Named { name: slot_name(self.u, s), id: self.slot(s), from, bytes });
        }
        for &y in &d.syms {
            v.push(Named { name: sym_name(self.u, y), id: self.sym(y), from: "", bytes: None });
        }
        v
    }

    /// The first identity that kept a taken digest unknown, using the comparison's memo.
    pub(crate) fn first_unresolved(&mut self, d: &BodyDigest) -> Option<String> {
        for &s in &d.slots {
            if self.slot(s).is_none() {
                return Some(slot_name(self.u, s));
            }
        }
        for &y in &d.syms {
            if self.sym(y).is_none() {
                return Some(sym_name(self.u, y));
            }
        }
        None
    }

    fn slot(&mut self, x: u32) -> Option<u128> {
        if let Some(r) = self.slots.get(&x) {
            return *r;
        }
        let r = self.ids.slot(self.u, x);
        self.slots.insert(x, r);
        r
    }

    /// A symbol's identity: what code reading it gets, not this run's number for it.
    fn sym(&mut self, id: u32) -> Option<u128> {
        if let Some(r) = self.syms.get(&id) {
            return *r;
        }
        // (a cycle through data objects resolves to unknown)
        self.syms.insert(id, None);
        let r = self.sym_uncached(id);
        self.syms.insert(id, r);
        r
    }

    fn sym_uncached(&mut self, id: u32) -> Option<u128> {
        let u = self.u;
        let mut h = Sip128::new();
        match u.syms.kind(id)? {
            SymKind::Rt(n) => {
                h.u8(1);
                h.str(n);
            }
            SymKind::Data(i) => {
                let (start, len, align, mutable) = u.syms.obj_info(i)?;
                // (a mutable object's contents are its readers' state, not a constant)
                if mutable {
                    return None;
                }
                let mut ptrs = crate::syms::obj_pointer_words(u, i)?;
                ptrs.sort_unstable_by_key(|p| p.0);
                let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, len as usize) };
                let mut words = Vec::with_capacity(ptrs.len());
                for (o, t) in ptrs {
                    words.push((o, self.sym(t.id)?, t.off));
                }
                return data_identity(bytes, align, u.syms.obj_name(i).as_deref(), &words);
            }
            SymKind::Static(d) => {
                h.u8(3);
                h.u128(self.ids.def(u, d)?);
            }
            SymKind::Foreign(n) => {
                h.u8(4);
                h.str(&n);
            }
            SymKind::Missing(n) => {
                h.u8(5);
                h.str(&n);
            }
            SymKind::StaticAddr(d) => {
                h.u8(6);
                h.u128(self.ids.def(u, d)?);
            }
            SymKind::Site(i) => {
                let sites = u.sites.lock().unwrap_or_else(|e| e.into_inner());
                let st = sites.get(i as usize)?;
                let (file, line, col) = st.location();
                return Some(site_identity(file, line, col, &st.msg));
            }
            SymKind::Thunk(x) => {
                h.u8(8);
                h.u128(self.slot(x)?);
            }
            SymKind::TierRec(x) => {
                h.u8(9);
                h.u128(self.slot(x)?);
            }
            SymKind::Rw(k) => {
                let (size, align) = *u.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner()).get(k as usize)?;
                h.u8(10);
                h.u64(size);
                h.u64(align);
            }
            SymKind::Provided(n) => {
                h.u8(11);
                h.str(&n);
            }
            SymKind::ThreadLocal(d) => {
                h.u8(12);
                h.u128(self.ids.def(u, d)?);
            }
            SymKind::SbThunk(x) => {
                h.u8(13);
                h.u128(self.slot(x)?);
            }
            SymKind::SbPlace(s) => {
                h.u8(14);
                h.u128(self.sym(s)?);
            }
            SymKind::SbTls(s) => {
                h.u8(15);
                h.u128(self.sym(s)?);
            }
            SymKind::SbTable => h.u8(16),
            SymKind::SbHost(n) => {
                h.u8(17);
                h.str(&n);
            }
        }
        Some(h.finish128())
    }
}

/// A constant data object's identity: its bytes with its pointer words zeroed, its alignment and
/// name, and per pointer word its offset, its target's identity and the target offset (a vtable:
/// the fns it points to).
pub(crate) fn data_identity(bytes: &[u8], align: u64, name: Option<&str>, words: &[(u32, u128, i64)]) -> Option<u128> {
    let mut b = bytes.to_vec();
    for (o, _, _) in words {
        b.get_mut(*o as usize..*o as usize + 8)?.fill(0);
    }
    let mut h = Sip128::new();
    h.u8(2);
    h.u64(align);
    match name {
        Some(n) => {
            h.u8(1);
            h.str(n);
        }
        None => h.u8(0),
    }
    h.write(&b);
    h.u32(words.len() as u32);
    for (o, t, off) in words {
        h.u32(*o);
        h.u128(*t);
        h.u64(*off as u64);
    }
    Some(h.finish128())
}

/// A panic site's identity: what its message and Location say (file path, line, column), not
/// this run's file version (an edit makes a new one) or byte offset.
pub(crate) fn site_identity(file: &[u8], line: u32, col: u32, msg: &str) -> u128 {
    let mut h = Sip128::new();
    h.u8(7);
    h.write(file);
    h.u32(line);
    h.u32(col);
    h.str(msg);
    h.finish128()
}

// ------------------------------------------------------------ records

/// A context whose slot is the code's own (an inlined recursive call).
const CTX_SELF: u32 = u32::MAX - 1;

/// Writes the digests `d` of code whose entry lists `inlined` (in the order written) to a record:
/// a flag and its own digest; per context its slot (an index into `inlined`, CTX_SELF, or NO_SLOT),
/// a flags byte (1 known, 2 can wait) and the digest. Unknown is written as unknown.
pub(crate) fn encode(resolve: &mut dyn FnMut(&Dg) -> Option<u128>, id: u32, d: Option<&CodeDigest>, inlined: &[u32], out: &mut Vec<u8>) {
    fn put(out: &mut Vec<u8>, known: Option<u128>, waits: bool) {
        out.push(known.is_some() as u8 | (waits as u8) << 1);
        out.extend_from_slice(&known.unwrap_or(0).to_le_bytes());
    }
    let Some(d) = d else {
        put(out, None, false);
        out.extend_from_slice(&0u32.to_le_bytes());
        return;
    };
    put(out, resolve(&d.own), false);
    out.extend_from_slice(&(d.ctx.len() as u32).to_le_bytes());
    for (s, g, waits) in d.ctx.iter() {
        let ix = if *s == id { CTX_SELF } else { inlined.iter().position(|x| x == s).map_or(NO_SLOT, |i| i as u32) };
        out.extend_from_slice(&ix.to_le_bytes());
        put(out, resolve(g), *waits);
    }
}

/// Reads what `encode` wrote. `slot_of(i)`: the slot of the record's i-th inlined key (None:
/// unknown here).
pub(crate) fn decode(b: &[u8], id: u32, slot_of: &dyn Fn(u32) -> Option<u32>) -> Option<Arc<CodeDigest>> {
    struct Rd<'a> {
        b: &'a [u8],
        p: usize,
    }
    impl Rd<'_> {
        fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
            let s = self.b.get(self.p..self.p + N)?;
            self.p += N;
            s.try_into().ok()
        }
        fn u32(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take::<4>()?))
        }
        fn dg(&mut self) -> Option<(Dg, bool)> {
            let [f] = self.take::<1>()?;
            let v = u128::from_le_bytes(self.take::<16>()?);
            Some((if f & 1 != 0 { Dg::Resolved(v) } else { Dg::Unknown }, f & 2 != 0))
        }
    }
    let mut d = Rd { b, p: 0 };
    let (own, _) = d.dg()?;
    let n = d.u32()? as usize;
    let mut ctx = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let ix = d.u32()?;
        let (g, waits) = d.dg()?;
        let s = match ix {
            CTX_SELF => id,
            NO_SLOT => NO_SLOT,
            i => slot_of(i).unwrap_or(NO_SLOT),
        };
        ctx.push((s, g, waits));
    }
    Some(crate::zone_alloc::kept_with(|| Arc::new(CodeDigest { own, ctx: ctx.into() })))
}

// ------------------------------------------------------------ the patch's count

/// A code version's start address and the digests kept for it.
pub(crate) type CodeVersion = (u64, Option<Arc<CodeDigest>>);

struct PatchCount {
    /// per slot this patch installed: the code it ran before and its new code
    builds: Vec<(u32, CodeVersion, CodeVersion)>,
    at: HashMap<u32, usize>,
    digest_ns: u64,
    closure_ordinal_file_misses: u64,
    stable_closure_name_file_misses: u64,
    /// RELAX_GATE_PROBE: the probed slots' lowerings before the edit's commit
    probes: Vec<Probe>,
}

/// A slot's lowering from the program's text now, outside a build: its body's digest, or why not.
type Lowered = Result<Arc<BodyDigest>, String>;

/// One name of a probe's listing: its identity, and for a slot where its key bytes came from.
pub(crate) struct Named {
    name: String,
    id: Option<u128>,
    from: &'static str,
    bytes: Option<Vec<u8>>,
}

/// A probed slot: its lowering before the edit's commit, and that lowering's identity and listing
/// as resolved then.
struct Probe {
    id: u32,
    before: Lowered,
    identity: Option<u128>,
    listing: Vec<Named>,
}

/// RELAX_GATE_PROBE=<part>[,<part>..]: the running slots whose names hold one of the parts are
/// lowered before an edit's commit and after it. Before: compared with the running code's own
/// digest (a mismatch with no edit is the run, not the edit). Across: per name, which identity
/// moved.
fn probe_parts() -> &'static [String] {
    static P: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var("RELAX_GATE_PROBE").map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default())
}

fn hex(x: Option<u128>) -> String {
    x.map_or("unknown".to_string(), |x| format!("{:012x}", x >> 80))
}

/// The names whose identities differ between two listings, position by position; for a slot,
/// where its key bytes came from on each side and the bytes from their first difference.
fn listing_diff(a: &[Named], b: &[Named]) -> Vec<String> {
    let side = |x: Option<&Named>| x.map_or("-".to_string(), |x| if x.from.is_empty() { format!("{} {}", x.name, hex(x.id)) } else { format!("{} {} ({})", x.name, hex(x.id), x.from) });
    let mut out = Vec::new();
    if a.len() != b.len() {
        out.push(format!("{} names before, {} after", a.len(), b.len()));
    }
    for k in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(k), b.get(k));
        if let (Some(x), Some(y)) = (x, y) {
            // (the same identity is the same name, whatever its slot or symbol number this run)
            if x.id.is_some() && x.id == y.id {
                continue;
            }
        }
        out.push(format!("#{k}: {} -> {}", side(x), side(y)));
        if let (Some(Some(p)), Some(Some(q))) = (x.map(|x| &x.bytes), y.map(|y| &y.bytes)) {
            let at = p.iter().zip(q.iter()).position(|(c, d)| c != d).unwrap_or(p.len().min(q.len()));
            let show = |v: &[u8]| v[at.min(v.len())..(at + 8).min(v.len())].iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
            out.push(format!("#{k}:   key bytes {} and {} long, first tag {} and {}, first difference at {}: [{}] -> [{}]", p.len(), q.len(),
                p.first().map_or("-".to_string(), |c| c.to_string()), q.first().map_or("-".to_string(), |c| c.to_string()), at, show(p), show(q)));
        }
    }
    out
}

fn with_resolver<R>(u: &Unit, f: impl FnOnce(&mut Resolver<'_, dyn Ids + '_>) -> R) -> R {
    match u.cache.as_ref() {
        Some(c) => f(&mut Resolver::new(u, unsafe { &mut *c.get() } as &mut dyn Ids)),
        None => f(&mut Resolver::new(u, &mut Session as &mut dyn Ids)),
    }
}

static PATCH: relax_session::sync::Mutex<Option<PatchCount>> = relax_session::sync::Mutex::new(None);

fn counting() -> bool {
    crate::reload::reload_trace()
}

fn gate_trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RELAX_GATE_TRACE").is_some())
}

/// A patch opens, before its edit commits: its installs are counted from here
/// (RELAX_RELOAD_TRACE), and RELAX_GATE_PROBE's slots are lowered from the text the running code
/// was built from.
pub(crate) unsafe fn patch_opened(up: *mut Unit) {
    if !counting() {
        return;
    }
    let mut probes = Vec::new();
    if !probe_parts().is_empty() {
        let ids: Vec<u32> = {
            let u = &*up;
            (0..u.fns.len() as u32).filter(|&i| u.fns[i as usize].code != 0 && { let n = u.fns[i as usize].name.to_string(); probe_parts().iter().any(|p| n.contains(p.as_str())) }).collect()
        };
        for id in ids {
            let before = crate::jit::gate_lower(up, id);
            let u = &*up;
            let (identity, listing) = with_resolver(u, |r| {
                // (listed first: a slot's key bytes and their source are read before any resolution
                // memoises its identity)
                let listing = before.as_ref().map(|x| r.listing(x)).unwrap_or_default();
                let run = running(u, id);
                let a = run.as_ref().and_then(|d| r.resolve(&d.own));
                let b = before.as_ref().ok().and_then(|x| r.resolve(&Dg::Taken(x.clone())));
                let verdict = match &before {
                    Err(e) => format!(" (not lowered: {e})"),
                    Ok(_) if a.is_some() && a == b => " -- same".to_string(),
                    Ok(_) => " -- DIFFERS".to_string(),
                };
                eprintln!("relax reload: gate probe, no edit: {}: running {} ({}), lowered now {}{verdict}", slot_name(u, id), hex(a), origin_name(u, u.fns[id as usize].code), hex(b));
                // (a running digest still taken can be split; a resolved one, from the cache, cannot)
                if let (Some(Dg::Taken(x)), Ok(_)) = (run.as_ref().map(|d| &d.own), &before) {
                    if a.is_none() || a != b {
                        for line in listing_diff(&r.listing(x), &listing) {
                            eprintln!("relax reload: gate probe, no edit:   running -> lowered now {line}");
                        }
                    }
                }
                (b, listing)
            });
            probes.push(Probe { id, before, identity, listing });
        }
    }
    PATCH_LOWER_NS.store(0, Ordering::Relaxed);
    *PATCH.lock().unwrap_or_else(|e| e.into_inner()) = Some(PatchCount {
        builds: Vec::new(), at: HashMap::new(), digest_ns: DIGEST_NS.load(Ordering::Relaxed),
        closure_ordinal_file_misses: CLOSURE_ORDINAL_FILE_MISSES.load(Ordering::Relaxed),
        stable_closure_name_file_misses: relax_front::program::STABLE_CLOSURE_NAME_FILE_MISSES.load(Ordering::Relaxed),
        probes,
    });
}

/// The running code's digests before a patch install (None when not counting).
pub(crate) fn before_install(u: &Unit, id: u32) -> Option<CodeVersion> {
    if !counting() || PATCH.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
        return None;
    }
    let at = if (id as usize) < u.fns.len() { u.fns[id as usize].code } else { 0 };
    Some((at, running(u, id)))
}

/// (trace) the patch's new code that no install path noted digests for, and what the slot held
/// at its install: the reason's origin (`origin_name`)
static UNNOTED: relax_session::sync::Mutex<Option<HashMap<u64, &'static str>>> = relax_session::sync::Mutex::new(None);

/// A patch installed new code for slot `id` at `at`; `old`: `before_install`'s answer.
pub(crate) fn installed(u: &Unit, id: u32, old: Option<CodeVersion>, at: u64) {
    let Some(old) = old else { return };
    let (new, noted) = {
        let code = u.code.read().unwrap_or_else(|e| e.into_inner());
        ((at, code.digest_at(at)), code.digest_origin_at(at).is_some())
    };
    if !noted && gate_trace() {
        let held = if u.fns[id as usize].rir.is_some() { "not noted; the slot holds its RIR" } else { "not noted; the slot held no RIR at its install" };
        UNNOTED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(at, held);
    }
    let mut g = PATCH.lock().unwrap_or_else(|e| e.into_inner());
    let Some(p) = g.as_mut() else { return };
    match p.at.get(&id) {
        // (installed again in the same patch: the first install's old code, the last one's new)
        Some(&k) => p.builds[k].2 = new,
        None => {
            p.at.insert(id, p.builds.len());
            p.builds.push((id, old, new));
        }
    }
}

/// A patch closes (on the owner thread): its installs classed as the gate would class them.
pub(crate) unsafe fn patch_closed(up: *mut Unit) {
    let Some(p) = PATCH.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    // the contexts of the running code that this patch built no code for: lowered from the new
    // text, once each, to compare with what the running code copied (never "absent means
    // unchanged": an always-inlined reader has no code of its own, so no walk selects it)
    let tl = relax_session::clock::cpu_ns();
    let misses = (CLOSURE_ORDINAL_FILE_MISSES.load(Ordering::Relaxed), relax_front::program::STABLE_CLOSURE_NAME_FILE_MISSES.load(Ordering::Relaxed));
    let mut lowered: HashMap<u32, Lowered> = HashMap::new();
    {
        let mut built: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for (id, _, (_, new)) in &p.builds {
            built.insert(*id);
            if let Some(n) = new {
                built.extend(n.ctx.iter().map(|c| c.0));
            }
        }
        let mut need: Vec<u32> = p.builds.iter().filter_map(|b| b.1 .1.as_ref()).flat_map(|o| o.ctx.iter().map(|c| c.0)).filter(|s| *s != NO_SLOT && !built.contains(s)).collect();
        need.sort_unstable();
        need.dedup();
        for s in need {
            lowered.insert(s, crate::jit::gate_lower(up, s));
        }
    }
    let lower_ns = relax_session::clock::cpu_ns().saturating_sub(tl);
    let u = &*up;
    let t = relax_session::clock::cpu_ns();
    let (line, names) = with_resolver(u, |r| classify(u, r, &p.builds, &lowered));
    let resolve_ns = relax_session::clock::cpu_ns().saturating_sub(t);
    eprintln!(
        "relax reload: gate: {line}; digests {:.1} ms at lowering this patch ({:.1} ms over {} bodies this run), resolving {:.1} ms, lowering {:.1} ms, {} contexts lowered for the check ({:.1} ms, {} not lowered), closure ordinal file misses {} ({} more in the check), stable closure name file misses {} ({} more in the check)",
        DIGEST_NS.load(Ordering::Relaxed).saturating_sub(p.digest_ns) as f64 / 1e6,
        DIGEST_NS.load(Ordering::Relaxed) as f64 / 1e6,
        DIGEST_N.load(Ordering::Relaxed),
        resolve_ns as f64 / 1e6,
        PATCH_LOWER_NS.load(Ordering::Relaxed) as f64 / 1e6,
        lowered.len(),
        lower_ns as f64 / 1e6,
        lowered.values().filter(|l| l.is_err()).count(),
        misses.0.saturating_sub(p.closure_ordinal_file_misses),
        CLOSURE_ORDINAL_FILE_MISSES.load(Ordering::Relaxed).saturating_sub(misses.0),
        misses.1.saturating_sub(p.stable_closure_name_file_misses),
        relax_front::program::STABLE_CLOSURE_NAME_FILE_MISSES.load(Ordering::Relaxed).saturating_sub(misses.1)
    );
    if gate_trace() {
        for (class, id) in names {
            let name = if (id as usize) < u.fns.len() { u.fns[id as usize].name.to_string() } else { "?".to_string() };
            eprintln!("relax reload: gate {class}: {name}");
        }
    }
    // the probes across the commit: per name, which identity moved (the before side as resolved
    // before the commit)
    for pr in &p.probes {
        let after = crate::jit::gate_lower(up, pr.id);
        let u = &*up;
        with_resolver(u, |r| {
            let listing = after.as_ref().map(|y| r.listing(y)).unwrap_or_default();
            let b = after.as_ref().ok().and_then(|x| r.resolve(&Dg::Taken(x.clone())));
            let verdict = match (&pr.before, &after) {
                (Err(e), _) | (_, Err(e)) => format!(" (not lowered: {e})"),
                _ if pr.identity.is_some() && pr.identity == b => " -- same".to_string(),
                _ => " -- DIFFERS".to_string(),
            };
            eprintln!("relax reload: gate probe, across the edit: {}: before {}, after {}{verdict}", slot_name(u, pr.id), hex(pr.identity), hex(b));
            if let (Ok(x), Ok(y)) = (&pr.before, &after) {
                if pr.identity.is_none() || pr.identity != b {
                    if x.raw != y.raw {
                        eprintln!("relax reload: gate probe, across the edit:   the body itself differs");
                    }
                    for line in listing_diff(&pr.listing, &listing) {
                        eprintln!("relax reload: gate probe, across the edit:   {line}");
                    }
                }
            }
        });
    }
    UNNOTED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

fn sym_name(u: &Unit, y: u32) -> String {
    let def = |d: crate::program::DefId| if (d.0 as usize) < u.prog.defs.len() { u.prog.def_path(d) } else { format!("def #{}", d.0) };
    let name = match u.syms.kind(y) {
        Some(SymKind::Rt(n)) => format!("Rt {n}"),
        Some(SymKind::Data(i)) => format!("Data #{i} {}", u.syms.obj_name(i).as_deref().unwrap_or("(unnamed)")),
        Some(SymKind::Static(d)) => format!("Static {}", def(d)),
        Some(SymKind::Foreign(n)) => format!("Foreign {n}"),
        Some(SymKind::Missing(n)) => format!("Missing {n}"),
        Some(SymKind::StaticAddr(d)) => format!("StaticAddr {}", def(d)),
        Some(SymKind::Site(i)) => format!("Site #{i}"),
        Some(SymKind::Thunk(s)) => format!("Thunk {}", slot_name(u, s)),
        Some(SymKind::TierRec(s)) => format!("TierRec {}", slot_name(u, s)),
        Some(SymKind::Rw(i)) => format!("Rw #{i}"),
        Some(SymKind::Provided(n)) => format!("Provided {n}"),
        Some(SymKind::ThreadLocal(d)) => format!("ThreadLocal {}", def(d)),
        Some(SymKind::SbThunk(s)) => format!("SbThunk {}", slot_name(u, s)),
        Some(SymKind::SbPlace(s)) => format!("SbPlace symbol #{s}"),
        Some(SymKind::SbTls(s)) => format!("SbTls symbol #{s}"),
        Some(SymKind::SbTable) => "SbTable".to_string(),
        Some(SymKind::SbHost(n)) => format!("SbHost {n}"),
        None => "no kind recorded".to_string(),
    };
    format!("symbol #{y} {name}")
}

fn slot_name(u: &Unit, id: u32) -> String {
    let name = if (id as usize) < u.fns.len() { &u.fns[id as usize].name } else { "?" };
    format!("slot #{id} {name}")
}

fn origin_name(u: &Unit, at: u64) -> &'static str {
    if let Some(o) = u.code.read().unwrap_or_else(|e| e.into_inner()).digest_origin_at(at) {
        return o.name();
    }
    UNNOTED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&at).copied()).unwrap_or("no origin recorded")
}

/// Called only for an unresolved digest while tracing; no diagnostic work on the count path.
fn unknown_digest<I: Ids + ?Sized>(u: &Unit, r: &mut Resolver<'_, I>, g: &Dg, at: u64) -> String {
    match g {
        Dg::Unknown => format!("recorded unknown; origin: {}", origin_name(u, at)),
        Dg::Taken(d) => format!("unresolved {}", r.first_unresolved(d).unwrap_or_else(|| "(no unresolved name found)".to_string())),
        Dg::Resolved(_) => "resolved".to_string(),
    }
}

/// Classes each install: changed (its own digest differs), a host (its own is equal, and a body
/// its running code copied changed or is unknown), digest-equal (dropped by the gate), unknown.
fn classify<I: Ids + ?Sized>(u: &Unit, r: &mut Resolver<'_, I>, builds: &[(u32, CodeVersion, CodeVersion)], lowered: &HashMap<u32, Lowered>) -> (String, Vec<(String, u32)>) {
    let traced = gate_trace();
    // the new digest of each body this patch's code took in, its own and its copies' (two
    // different ones: unknown)
    let mut new_of: HashMap<u32, Option<u128>> = HashMap::new();
    let mut new_unknown = HashMap::new();
    let mut put = |s: u32, g: &Dg, at: u64| {
        let v = r.resolve(g);
        if s == NO_SLOT {
            return;
        }
        if traced && v.is_none() {
            new_unknown.entry(s).or_insert_with(|| format!("new digest for {}: {}", slot_name(u, s), unknown_digest(u, r, g, at)));
        }
        let e = new_of.entry(s).or_insert(v);
        if *e != v {
            *e = None;
            if traced {
                new_unknown.entry(s).or_insert_with(|| format!("two different new digests for {}", slot_name(u, s)));
            }
        }
    };
    for (id, _, (at, new)) in builds {
        if let Some(n) = new {
            put(*id, &n.own, *at);
            for (s, g, _) in n.ctx.iter() {
                put(*s, g, *at);
            }
        }
    }
    let (mut d, mut h, mut hu, mut c, mut un) = (0, 0, 0, 0, 0);
    let mut names = Vec::new();
    for (id, (old_at, old), (new_at, new)) in builds {
        let own = match (old, new) {
            (Some(o), Some(n)) => (r.resolve(&o.own), r.resolve(&n.own)),
            _ => (None, None),
        };
        let class = match own {
            (Some(a), Some(b)) if a != b => {
                c += 1;
                // (traced) what differs: the body itself, or what it names
                if traced {
                    let same = |x: bool| if x { "same" } else { "differs" };
                    names.push((match (&old.as_ref().unwrap().own, &new.as_ref().unwrap().own) {
                        (Dg::Taken(x), Dg::Taken(y)) => format!("changed (body {}, slots {} [{} -> {}], symbols {} [{} -> {}])", same(x.raw == y.raw),
                            same(x.slots == y.slots), x.slots.len(), y.slots.len(), same(x.syms == y.syms), x.syms.len(), y.syms.len()),
                        (Dg::Resolved(_), _) => "changed (the running code's digests came from the cache)".to_string(),
                        _ => "changed".to_string(),
                    }, *id));
                    continue;
                }
                "changed"
            }
            (Some(_), Some(_)) => {
                let o = old.as_ref().unwrap();
                let mut changed = false;
                let mut unknown = false;
                let mut reason = None;
                for (s, g, _) in o.ctx.iter() {
                    match (r.resolve(g), new_of.get(s)) {
                        _ if *s == NO_SLOT => {
                            unknown = true;
                            if traced && reason.is_none() { reason = Some("a context without a slot".to_string()); }
                        }
                        (None, _) => {
                            unknown = true;
                            if traced && reason.is_none() { reason = Some(unknown_digest(u, r, g, *old_at)); }
                        }
                        (_, Some(None)) => {
                            unknown = true;
                            if traced && reason.is_none() { reason = Some(new_unknown.get(s).cloned().unwrap_or_else(|| format!("no new digest for {}", slot_name(u, *s)))); }
                        }
                        (Some(a), Some(Some(b))) if a != *b => changed = true,
                        // no code of this patch took it in: its lowering from the new text decides
                        (Some(a), None) => match lowered.get(s) {
                            Some(Ok(b)) => match r.resolve(&Dg::Taken(b.clone())) {
                                Some(x) if x != a => changed = true,
                                Some(_) => {}
                                None => {
                                    unknown = true;
                                    if traced && reason.is_none() { reason = Some(format!("{} lowered now: unresolved {}", slot_name(u, *s), r.first_unresolved(b).unwrap_or_else(|| "(no unresolved name found)".to_string()))); }
                                }
                            },
                            Some(Err(e)) => {
                                unknown = true;
                                if traced && reason.is_none() { reason = Some(format!("{} not lowered: {e}", slot_name(u, *s))); }
                            }
                            // (the check lowers every context the patch built nothing for)
                            None => {
                                unknown = true;
                                if traced && reason.is_none() { reason = Some(format!("{} built by this patch without a digest", slot_name(u, *s))); }
                            }
                        },
                        _ => {}
                    }
                }
                if changed {
                    h += 1;
                    "host"
                } else if unknown {
                    h += 1;
                    hu += 1;
                    if let Some(reason) = reason {
                        names.push((format!("host (unknown context: {reason})"), *id));
                        continue;
                    }
                    "host (unknown context)"
                } else {
                    d += 1;
                    "equal"
                }
            }
            _ => {
                un += 1;
                if traced {
                    let reason = match (old, new) {
                        (None, _) => format!("running code has no digest; origin: {}", origin_name(u, *old_at)),
                        (_, None) => format!("new code has no digest; origin: {}", origin_name(u, *new_at)),
                        (Some(o), _) if own.0.is_none() => format!("running code: {}", unknown_digest(u, r, &o.own, *old_at)),
                        (_, Some(n)) => format!("new code: {}", unknown_digest(u, r, &n.own, *new_at)),
                    };
                    names.push((format!("unknown ({reason})"), *id));
                    continue;
                }
                "unknown"
            }
        };
        if traced {
            names.push((class.to_string(), *id));
        }
    }
    (format!("{} selected, {d} digest-equal, {h} hosts ({hu} by an unknown context), {c} changed, {un} unknown", builds.len()), names)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(d: &CodeDigest) -> (Option<u128>, Vec<(u32, Option<u128>, bool)>) {
        let v = |g: &Dg| match g {
            Dg::Resolved(x) => Some(*x),
            _ => None,
        };
        (v(&d.own), d.ctx.iter().map(|(s, g, w)| (*s, v(g), *w)).collect())
    }

    /// A record's digest tail round-trips: its own digest; per context its slot (by the position
    /// of its key in the entry's inlined list, its own code, or unknown), its digest and its wait
    /// bit. A cut-off tail reads as unknown, and code without digests as unknown digests.
    #[test]
    fn a_records_digests_round_trip_and_a_cut_tail_is_unknown() {
        let d = CodeDigest { own: Dg::Resolved(7), ctx: vec![(40, Dg::Resolved(8), true), (3, Dg::Unknown, false), (99, Dg::Resolved(9), false), (NO_SLOT, Dg::Resolved(10), true)].into() };
        let mut out = Vec::new();
        let resolved = &mut |g: &Dg| match g {
            Dg::Resolved(x) => Some(*x),
            _ => None,
        };
        encode(resolved, 3, Some(&d), &[50, 40], &mut out);
        let back = decode(&out, 3, &|i| [50, 40].get(i as usize).copied()).unwrap();
        assert_eq!(flat(&back), (Some(7), vec![(40, Some(8), true), (3, None, false), (NO_SLOT, Some(9), false), (NO_SLOT, Some(10), true)]), "a context outside the entry's inlined keys has no slot");
        for end in 0..out.len() {
            assert!(decode(&out[..end], 3, &|_| None).is_none(), "a tail cut at {end} of {} decoded", out.len());
        }
        let mut none = Vec::new();
        encode(resolved, 3, None, &[], &mut none);
        assert_eq!(flat(&decode(&none, 3, &|_| None).unwrap()), (None, vec![]));
    }

    /// A symbol is named by what code reading it gets: a panic site by its message and location
    /// (not its file version), a vtable by its bytes and the fns it points to (not this run's
    /// addresses in its pointer words).
    #[test]
    fn panic_sites_and_vtables_are_named_by_their_content() {
        let site = site_identity(b"src/a.rs\0", 3, 9, "boom");
        assert_eq!(site, site_identity(b"src/a.rs\0", 3, 9, "boom"));
        assert_ne!(site, site_identity(b"src/a.rs\0", 3, 9, "bang"), "a changed panic message");
        assert_ne!(site, site_identity(b"src/a.rs\0", 4, 9, "boom"), "a moved panic");
        let mut bytes = [0u8; 32];
        bytes[8] = 16;
        bytes[16] = 8;
        let words = [(0, 100u128, 0i64), (24, 101, 0)];
        let vt = data_identity(&bytes, 8, Some("vtable"), &words).unwrap();
        let mut addrs = bytes;
        addrs[0] = 0xff;
        addrs[24] = 0x7f;
        assert_eq!(Some(vt), data_identity(&addrs, 8, Some("vtable"), &words), "its pointer words hold this run's addresses");
        assert_ne!(Some(vt), data_identity(&bytes, 8, Some("vtable"), &[(0, 100, 0), (24, 102, 0)]), "a method points to another fn");
        let mut size = bytes;
        size[8] = 24;
        assert_ne!(Some(vt), data_identity(&size, 8, Some("vtable"), &words), "its size changed");
        assert_eq!(data_identity(&bytes, 8, None, &[(28, 1, 0)]), None, "a pointer word past the object");
    }
}

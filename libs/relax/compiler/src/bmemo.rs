//! The back-end memo (lane POLYMORPH): a split compile's back end (inlining, optimisation, code
//! generation; `BackWork::run`) is a pure function of the front's preparation. It reads no unit
//! state: the root body, the inline candidates' bodies, the deferred candidates' preparations,
//! the nounwind facts, the pass configuration and the read-only data reader (`Prep`), plus the
//! code generator's settings. Function slots are opaque names in it: it compares them, never
//! looks them up. So two preparations equal up to a renaming of their slots make the same code
//! with the same renaming of its relocations, and the same optimised RIR.
//!
//! Instances of one generic fn mostly differ only in the slots they call (`Vec<A>::reserve`
//! calls `Vec<A>::try_grow_to`, `Vec<B>::reserve` calls `Vec<B>::try_grow_to`): their bodies
//! depend on the type arguments through layout constants, which stay in the key as they are.
//! A cold level load builds ~24k bodies of ~4k generic fns; a body that matches an earlier one
//! up to slot names takes that one's back end result, renamed, instead of running it again.
//!
//! Exact by construction: the key is the whole preparation with every slot id replaced by its
//! index of first occurrence (the slot being built is index 0), compared in full (the hash only
//! picks the bucket); every other field -- constants, symbols (data, vtables, thunks named by
//! symbol), positions, layout facts, flags -- stays as it is. A result naming a slot the input
//! does not name (none is known to exist) is not stored. Each slot keeps its own code copy, its
//! own thunk and its own RIR: function pointers stay distinct, tier-ups, patches and the disk
//! cache see an ordinary build.
//!
//! Lifetime: compiler generation (taken at the zone swap with the other compile statics), keyed
//! by the live-patch epoch the front ran under (a patch can change read-only data a body folds),
//! kept in two generations of MAX_BYTES / 2 (estimated) each: when the newer one fills, the older
//! one is dropped. Only slots whose body can have siblings take part (instances of generic fns,
//! their closures, glue, shims and SPEC-V twins; `worthy`). Off with MAKEPAD_RUN_BMEMO=0, and whenever a
//! name-selected code generator option is set (the root body's name is not part of the key).

use super::*;
use relax_backend::rir::{Func, Reloc, RelocAt};
use std::sync::Arc;

/// Estimated bytes of entries before the memo is dropped and starts again (instances of one
/// generic fn mostly come in one burst: a page's first visit, a level load).
const MAX_BYTES: usize = 64 << 20;

/// What an entry owns, roughly: its copies of bodies (the root, the optimised result, deferred
/// results; candidates' bodies are shared Arcs, counted by the unit), its code and tables.
fn entry_bytes(e: &Entry) -> usize {
    fn body(f: &Func) -> usize {
        f.blocks.iter().map(|b| b.insts.len() * (std::mem::size_of::<relax_backend::rir::Inst>() + 8) + std::mem::size_of::<relax_backend::rir::Block>()).sum::<usize>() + f.vregs.len() + 128
    }
    fn prep(p: &SPrep) -> usize {
        body(&p.func) + p.callees.len() * 24 + p.deferred.iter().map(|d| prep(&d.2)).sum::<usize>()
    }
    prep(&e.prep)
        + e.opt.as_ref().map_or(0, |o| body(&o.0))
        + e.done.iter().map(|d| d.1.as_ref().map_or(0, |x| body(&x.0))).sum::<usize>()
        + e.code.len()
        + e.relocs.len() * std::mem::size_of::<RelocAt>()
        + e.map.len() * 16
        + e.canon.order.len() * 12
}

/// slot -> index of first occurrence (the slot being built is 0)
pub(crate) struct Canon {
    ix: crate::fxhash::HashMap<u32, u32>,
    order: Vec<u32>,
}

impl Canon {
    fn new(id: u32) -> Canon {
        let mut c = Canon { ix: Default::default(), order: Vec::new() };
        c.add(id);
        c
    }
    fn add(&mut self, s: u32) -> u32 {
        let n = self.order.len() as u32;
        *self.ix.entry(s).or_insert_with(|| {
            self.order.push(s);
            n
        })
    }
    /// the index of a slot the input named (None: it named none such)
    fn get(&self, s: u32) -> Option<u32> {
        self.ix.get(&s).copied()
    }
    fn map(&self) -> impl FnMut(u32) -> u32 + '_ {
        |s| self.get(s).unwrap_or(u32::MAX)
    }
}

/// Hashes a preparation (minus its name) with its slots numbered by first occurrence into `c`.
fn hash_prep<H: std::hash::Hasher>(p: &Prep, c: &mut Canon, h: &mut H) {
    use std::hash::Hash;
    p.func.hash_body_mapped(h, &mut |s| c.add(s));
    p.callees.len().hash(h);
    for (s, f, a) in &p.callees {
        (c.add(*s), a).hash(h);
        f.hash_body_mapped(h, &mut |s| c.add(s));
    }
    p.facts.len().hash(h);
    for (s, v) in &p.facts {
        (c.add(*s), v).hash(h);
    }
    (p.cfg, p.ro.as_ref().map(|r| (r.read as usize, r.ctx)), p.relayout).hash(h);
    p.deferred.len().hash(h);
    for (s, a, q) in &p.deferred {
        (c.add(*s), a).hash(h);
        hash_prep(q, c, h);
    }
}

/// What a stored back end got: the preparation (minus its name), slots as the build named them.
pub(crate) struct SPrep {
    func: Func,
    callees: Vec<(u32, Arc<Func>, u8)>,
    facts: Vec<(u32, bool)>,
    cfg: relax_codegen::opt::OptCfg,
    ro: Option<(usize, usize)>,
    relayout: bool,
    deferred: Vec<(u32, Option<u8>, SPrep)>,
}

impl SPrep {
    pub(crate) fn of(p: &Prep) -> SPrep {
        SPrep {
            func: p.func.clone(),
            callees: p.callees.clone(),
            facts: p.facts.clone(),
            cfg: p.cfg,
            ro: p.ro.as_ref().map(|r| (r.read as usize, r.ctx)),
            relayout: p.relayout,
            deferred: p.deferred.iter().map(|(s, a, q)| (*s, *a, SPrep::of(q))).collect(),
        }
    }
}

/// `p` (slots numbered by `cp`) equals `s` (numbered by `cs`) up to slot names.
fn same(p: &Prep, cp: &Canon, s: &SPrep, cs: &Canon) -> bool {
    let (mut mp, mut ms) = (cp.map(), cs.map());
    p.func.same_body_mapped(&mut mp, &s.func, &mut ms)
        && p.callees.len() == s.callees.len()
        && p.callees.iter().zip(&s.callees).all(|(a, b)| mp(a.0) == ms(b.0) && a.2 == b.2 && a.1.same_body_mapped(&mut mp, &b.1, &mut ms))
        && p.facts.len() == s.facts.len()
        && p.facts.iter().zip(&s.facts).all(|(a, b)| mp(a.0) == ms(b.0) && a.1 == b.1)
        && p.cfg == s.cfg
        && p.ro.as_ref().map(|r| (r.read as usize, r.ctx)) == s.ro
        && p.relayout == s.relayout
        && p.deferred.len() == s.deferred.len()
        && p.deferred.iter().zip(&s.deferred).all(|(a, b)| mp(a.0) == ms(b.0) && a.1 == b.1 && same(&a.2, cp, &b.2, cs))
}

/// What the back end got: the preparation's hash and slot numbering, and the code generator's
/// settings (fold, rotate, opt_regs, patch epoch).
pub(crate) struct Key {
    hash: u64,
    settings: (bool, bool, bool, u64),
    canon: Canon,
    /// the root's and the deferred candidates' names (index -> name): results are named by them
    names: Vec<(u32, String)>,
}

fn names(p: &Prep, ix: u32, c: &Canon, out: &mut Vec<(u32, String)>) {
    out.push((ix, p.func.name.clone()));
    for (s, _, q) in &p.deferred {
        if let Some(i) = c.get(*s) {
            names(q, i, c, out);
        }
    }
}

/// One stored back end result, slots renamed to indices.
struct Entry {
    prep: SPrep,
    canon: Canon,
    settings: (bool, bool, bool, u64),
    opt: Option<(Func, Vec<(u32, usize)>, u64)>,
    code: Vec<u8>,
    map: Vec<(u32, u64)>,
    relocs: Vec<RelocAt>,
    cg_ns: u64,
    done: Vec<(u32, Result<(Func, Vec<(u32, usize)>), String>)>,
    frame: (u32, u32),
    /// thread cpu the back end took (what a hit saves)
    cpu_ns: u64,
}

pub(crate) static MEMO: std::sync::LazyLock<relax_session::sync::Mutex<Memo>> = std::sync::LazyLock::new(|| relax_session::sync::Mutex::new(Memo::default()));

#[derive(Default)]
pub(crate) struct Memo {
    map: HashMap<u64, Vec<Arc<Entry>>>,
    /// the generation before `map` (looked up too; dropped when `map` fills)
    old: HashMap<u64, Vec<Arc<Entry>>>,
    len: usize,
    bytes: usize,
    epoch: u64,
}

impl Memo {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// hits, misses, results not stored, cpu ns the hits saved, cpu ns spent on keys and lookups,
/// back ends of a slot whose RIR existed (code generation only: not memoized), times the memo
/// was dropped full
pub(crate) static STATS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// The memo is used: not MAKEPAD_RUN_BMEMO=0, and no code generator option selects functions by
/// name (the key leaves the name out), no interpreter selection, no web export or back-end bench
/// capture (they record what the back ends that run see).
pub(crate) fn on(options: &crate::options::Options) -> bool {
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENV.get_or_init(|| std::env::var("MAKEPAD_RUN_BMEMO").map_or(true, |v| v != "0") && std::env::var_os("MAKEPAD_RUN_INTERP").is_none())
        && options.no_layout_fn.is_none()
        && options.layout_only.is_none()
        && options.unroll_only.is_none()
        && !options.no_codegen
        && !crate::wasm_export::enabled()
        && !backend_bench_on()
}

/// Whether slot `id`'s back end may share with another slot's (unit call): an instance with type
/// arguments, a closure inside one, glue, a shim, an unverified-mode twin. A plain fn's body is
/// its own (another body equal to it up to slot names is rare): not worth a key or an entry.
pub(crate) fn worthy(u: &Unit, id: u32) -> bool {
    match &u.fns[id as usize].key {
        FnKey::Inst(_, args) => !args.is_empty(),
        FnKey::Closure(_, _, parent) => (*parent as usize) < u.fns.len() && *parent != id && worthy(u, *parent),
        FnKey::Glue(..) | FnKey::Shim(_) | FnKey::Plain(_) => true,
        FnKey::Sandbox(_) => false,
    }
}

/// The key of a back end over `p` for slot `id`.
pub(crate) fn key(id: u32, p: &Prep, settings: (bool, bool, bool, u64)) -> Key {
    let t0 = cpu_ns();
    let mut canon = Canon::new(id);
    let mut h = crate::fxhash::FxHasher::default();
    hash_prep(p, &mut canon, &mut h);
    std::hash::Hash::hash(&settings, &mut h);
    let hash = std::hash::Hasher::finish(&h);
    let mut n = Vec::new();
    names(p, 0, &canon, &mut n);
    STATS[4].fetch_add(cpu_ns() - t0, Ordering::Relaxed);
    Key { hash, settings, canon, names: n }
}

/// What a hit hands the back end's caller (BackDone's parts), slots renamed to the instance's.
pub(crate) type Hit = (Option<(Arc<Func>, Vec<(u32, usize)>, u64)>, Vec<u8>, Vec<(u32, u64)>, Vec<RelocAt>, u64, Vec<Deferred>, (u32, u32));

pub(crate) fn lookup(k: &Key, p: &Prep) -> Option<Hit> {
    let t0 = cpu_ns();
    let found: Vec<Arc<Entry>> = {
        let g = MEMO.lock().unwrap_or_else(|e| e.into_inner());
        if g.epoch != k.settings.3 {
            return None;
        }
        let mut v = g.map.get(&k.hash).cloned().unwrap_or_default();
        v.extend(g.old.get(&k.hash).into_iter().flatten().cloned());
        v
    };
    let e = found.into_iter().find(|e| e.settings == k.settings && same(p, &k.canon, &e.prep, &e.canon));
    STATS[4].fetch_add(cpu_ns() - t0, Ordering::Relaxed);
    let e = e?;
    let back = |s: &mut u32| *s = k.canon.order[*s as usize];
    let name = |ix: u32| k.names.iter().find(|x| x.0 == ix).map(|x| x.1.clone()).unwrap_or_default();
    let func = |f: &Func, ix: u32| {
        let mut f = f.clone();
        f.fn_slots_mut(&mut |s| back(s));
        f.nounwind_inputs.sort_unstable();
        f.name = name(ix);
        f
    };
    let inl = |v: &[(u32, usize)]| v.iter().map(|&(s, r)| (k.canon.order[s as usize], r)).collect::<Vec<_>>();
    let opt = e.opt.as_ref().map(|(f, i, ns)| (Arc::new(func(f, 0)), inl(i), *ns));
    let relocs = e.relocs.iter().map(|r| {
        let mut r = *r;
        if let Reloc::Slot(s) | Reloc::Thunk(s) | Reloc::Call(s) = &mut r.target {
            back(s);
        }
        r
    }).collect();
    let done = e.done.iter().map(|(c, r)| (k.canon.order[*c as usize], match r {
        Ok((f, i)) => Ok((func(f, *c), inl(i))),
        Err(s) => Err(s.clone()),
    })).collect();
    STATS[0].fetch_add(1, Ordering::Relaxed);
    STATS[3].fetch_add(e.cpu_ns, Ordering::Relaxed);
    Some((opt, e.code.clone(), e.map.clone(), relocs, e.cg_ns, done, e.frame))
}

/// Stores a back end's result under its key (not when it names a slot the input does not).
pub(crate) fn store(k: Key, prep: SPrep, opt: &Option<(Arc<Func>, Vec<(u32, usize)>, u64)>, code: &[u8], map: &[(u32, u64)], relocs: &[RelocAt], cg_ns: u64, done: &[Deferred], frame: (u32, u32), cpu: u64) {
    STATS[1].fetch_add(1, Ordering::Relaxed);
    let c = &k.canon;
    let ok = std::cell::Cell::new(true);
    let to = |s: &mut u32| match c.get(*s) {
        Some(i) => *s = i,
        None => ok.set(false),
    };
    let func = |f: &Func| {
        let mut f = f.clone();
        f.fn_slots_mut(&mut |s| to(s));
        f.name = String::new();
        f
    };
    let inl = |v: &[(u32, usize)]| v.iter().map(|&(s, r)| {
        let mut s = s;
        to(&mut s);
        (s, r)
    }).collect::<Vec<_>>();
    let opt = opt.as_ref().map(|(f, i, ns)| (func(f), inl(i), *ns));
    let done: Vec<_> = done.iter().map(|(s, r)| {
        let mut s = *s;
        to(&mut s);
        (s, match r {
            Ok((f, i)) => Ok((func(f), inl(i))),
            Err(e) => Err(e.clone()),
        })
    }).collect();
    let relocs: Vec<RelocAt> = relocs.iter().map(|r| {
        let mut r = *r;
        if let Reloc::Slot(s) | Reloc::Thunk(s) | Reloc::Call(s) = &mut r.target {
            to(s);
        }
        r
    }).collect();
    if !ok.get() {
        STATS[2].fetch_add(1, Ordering::Relaxed);
        return;
    }
    let e = Arc::new(Entry { prep, canon: k.canon, settings: k.settings, opt, code: code.to_vec(), map: map.to_vec(), relocs, cg_ns, done, frame, cpu_ns: cpu });
    let mut g = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    let n = entry_bytes(&e);
    if g.epoch != k.settings.3 {
        g.map = HashMap::new();
        g.old = HashMap::new();
        g.len = 0;
        g.bytes = 0;
        g.epoch = k.settings.3;
    } else if g.bytes + n > MAX_BYTES / 2 {
        // the full generation becomes the old one, the old one goes
        STATS[6].fetch_add(1, Ordering::Relaxed);
        g.old = std::mem::take(&mut g.map);
        g.len = 0;
        g.bytes = 0;
    }
    g.len += 1;
    g.bytes += n;
    g.map.entry(k.hash).or_default().push(e);
}

/// One line for the burst summaries: hits / builds, the cpu ms the hits saved, key cpu ms.
pub(crate) fn line() -> String {
    let s = |i: usize| STATS[i].load(Ordering::Relaxed);
    format!("back-end memo: {} hits of {} builds ({} not stored), saved {:.0} ms cpu, keys {:.0} ms; {} codegen-only builds; dropped full {} times, {:.1} MB now", s(0), s(0) + s(1), s(2), s(3) as f64 / 1e6, s(4) as f64 / 1e6, s(5), s(6), MEMO.lock().unwrap_or_else(|e| e.into_inner()).bytes as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relax_backend::rir::{Callee, Cls, Inst, RelocKind, Sym, Term};

    /// the tests share MEMO (an epoch change empties it)
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A body calling `a` then `b`, taking the address of `a`, plus `k`: the layout constant.
    fn body(name: &str, a: u32, b: u32, k: i64) -> Func {
        let mut f = Func::new(name.to_string(), 1);
        let bl = f.block();
        let v = f.vreg(Cls::I);
        let w = f.vreg(Cls::I);
        f.blocks[bl as usize].insts.push(Inst::Iconst(v, k));
        f.blocks[bl as usize].insts.push(Inst::Call(Callee::Fn(a), vec![v], vec![]));
        f.blocks[bl as usize].insts.push(Inst::Call(Callee::Fn(b), vec![v], vec![]));
        f.blocks[bl as usize].insts.push(Inst::FnAddr(w, a));
        f.blocks[bl as usize].insts.push(Inst::Addr(w, Sym::new(7)));
        f.blocks[bl as usize].pos = vec![0; 5];
        f.blocks[bl as usize].term = Term::Ret(vec![]);
        f.nounwind_inputs = vec![(a, false), (b, true)];
        f.nounwind_inputs.sort_unstable();
        f
    }

    fn prep(f: Func, callee: Option<(u32, Func)>) -> Prep {
        let cfg = relax_codegen::opt::OptCfg { inline: true, inline_all: true, fold: true, cse: true, sroa: true, licm: true, imm: true, layout: true };
        Prep { func: f, cfg, callees: callee.into_iter().map(|(s, f)| (s, Arc::new(f), 0u8)).collect(), ro: None, relayout: false, name: String::new(), deferred: Vec::new(), facts: Vec::new() }
    }

    #[test]
    fn same_body_up_to_slot_names_shares_the_result_renamed() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let settings = (true, true, false, 0xbeef);
        // instance A: slot 3 calls 10 and 11; instance B: slot 4 calls 20 and 21 (same constants)
        let pa = prep(body("f<A>", 10, 11, 8), Some((11, body("g<A>", 30, 3, 1))));
        let pb = prep(body("f<B>", 20, 21, 8), Some((21, body("g<B>", 40, 4, 1))));
        let (ka, kb) = (key(3, &pa, settings), key(4, &pb, settings));
        assert_eq!(ka.hash, kb.hash);
        assert!(same(&pb, &kb.canon, &SPrep::of(&pa), &ka.canon));
        // a different layout constant, or a non-bijective naming (both calls to one slot): no match
        let pc = prep(body("f<C>", 50, 51, 16), Some((51, body("g<C>", 60, 5, 1))));
        assert!(!same(&pc, &key(5, &pc, settings).canon, &SPrep::of(&pa), &ka.canon));
        let pd = prep(body("f<D>", 70, 70, 8), Some((70, body("g<D>", 80, 6, 1))));
        assert!(!same(&pd, &key(6, &pd, settings).canon, &SPrep::of(&pa), &ka.canon));
        // A's result: optimised body + code calling 10, 11 and itself
        let opt = Some((Arc::new(body("f<A>", 10, 11, 8)), vec![(11u32, 2usize)], 5u64));
        let relocs = vec![
            RelocAt { at: 0, kind: RelocKind::X64Branch32, target: Reloc::Call(10) },
            RelocAt { at: 8, kind: RelocKind::Pool64, target: Reloc::Thunk(3) },
            RelocAt { at: 16, kind: RelocKind::Pool64, target: Reloc::Sym(Sym::new(7)) },
        ];
        let sa = SPrep::of(&pa);
        store(ka, sa, &opt, &[1, 2, 3], &[(0, 9)], &relocs, 1, &[], (32, 8), 100);
        let (o, code, map, r, _, done, frame) = lookup(&kb, &pb).expect("hit");
        assert_eq!(code, vec![1, 2, 3]);
        assert_eq!(map, vec![(0, 9)]);
        assert_eq!(frame, (32, 8));
        assert!(done.is_empty());
        assert_eq!(r[0].target, Reloc::Call(20));
        assert_eq!(r[1].target, Reloc::Thunk(4));
        assert_eq!(r[2].target, Reloc::Sym(Sym::new(7)));
        let (f, inl, _) = o.unwrap();
        assert_eq!(f.name, "f<B>");
        let mut id = |s: u32| s;
        let mut id2 = |s: u32| s;
        assert!(f.same_body_mapped(&mut id, &body("f<B>", 20, 21, 8), &mut id2));
        assert_eq!(inl, vec![(21, 2)]);
        // the other constant does not hit; another patch epoch does not hit
        assert!(lookup(&key(5, &pc, settings), &pc).is_none());
        let ke = key(4, &pb, (true, true, false, 0xbef0));
        assert!(lookup(&ke, &pb).is_none());
    }

    #[test]
    fn a_result_naming_a_slot_the_input_does_not_is_not_stored() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let settings = (false, true, false, 0x1234);
        let pa = prep(body("h<A>", 10, 11, 4), None);
        let pb = prep(body("h<B>", 20, 21, 4), None);
        let ka = key(3, &pa, settings);
        let relocs = vec![RelocAt { at: 0, kind: RelocKind::X64Branch32, target: Reloc::Call(99) }];
        store(ka, SPrep::of(&pa), &None, &[1], &[], &relocs, 1, &[], (0, 0), 1);
        assert!(lookup(&key(4, &pb, settings), &pb).is_none());
    }
}

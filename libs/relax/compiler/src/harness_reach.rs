//! Live test entry marks. Only executed slots and their compiled inline trees contribute.

use crate::jit::{FnKey, Unit};
use crate::program::{DefId, DefKind};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::*};

/// A byte per slot, in 64K banks: the full u32 slot space without a fixed function limit.
const BANK: usize = 1 << 16;
pub(crate) const TLS_ENTERED: u32 = 24;

struct Bitmap {
    table: Box<[AtomicU64]>,
    banks: HashMap<usize, Box<[AtomicU8]>>,
}

impl Bitmap {
    fn new(banks: &HashSet<usize>) -> Self {
        let mut b = Self { table: (0..BANK).map(|_| AtomicU64::new(0)).collect(), banks: HashMap::new() };
        for &bank in banks { b.grow(bank); }
        b
    }

    fn grow(&mut self, bank: usize) {
        if self.banks.contains_key(&bank) { return; }
        let bytes: Box<[AtomicU8]> = (0..BANK).map(|_| AtomicU8::new(0)).collect();
        self.table[bank].store(bytes.as_ptr() as u64, Release);
        self.banks.insert(bank, bytes);
    }

    fn read(&self, slots: &mut HashSet<u32>, clear: bool) {
        for (&bank, bytes) in &self.banks {
            for (i, byte) in bytes.iter().enumerate() {
                if byte.load(Relaxed) != 0 {
                    slots.insert((bank * BANK + i) as u32);
                    if clear { byte.store(0, Relaxed); }
                }
            }
        }
    }
}

struct Capture {
    index: usize,
    complete: AtomicBool,
    invalid: AtomicBool,
    /// Includes children reserved before the OS starts them and thread-local destructors.
    active: AtomicUsize,
    slots: Mutex<HashSet<u32>>,
}

struct Thread {
    bitmap: Bitmap,
    test: Option<Arc<Capture>>,
}

#[derive(Default)]
struct State {
    banks: HashSet<usize>,
    threads: HashMap<u64, Thread>,
    tests: Vec<Option<Arc<Capture>>>,
    unattributed: HashSet<u32>,
    /// Union across installed versions; old frames may still enter their inlinees.
    inlined: HashMap<u32, HashSet<u32>>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> { STATE.get_or_init(|| Mutex::new(State::default())) }

pub(super) fn pending(index: usize) {
    if let Some(t) = &state().lock().unwrap().tests[index] { t.complete.store(false, Release); }
}

pub(super) fn reset(len: usize) {
    let _k = crate::zone_alloc::kept();
    state().lock().unwrap().tests = (0..len).map(|_| None).collect();
}

/// Publish banks before any code naming their slots can run.
pub(crate) fn slot(id: u32) {
    let _k = crate::zone_alloc::kept();
    let mut s = state().lock().unwrap();
    let bank = id as usize / BANK;
    if s.banks.insert(bank) {
        for thread in s.threads.values_mut() { thread.bitmap.grow(bank); }
    }
}

pub(crate) fn note(id: u32, deps: impl Iterator<Item = u32>) {
    if !super::live::enabled() { return; }
    let _k = crate::zone_alloc::kept();
    state().lock().unwrap().inlined.entry(id).or_default().extend(deps.filter(|&c| c != id));
}

pub(crate) fn init(block: u64) {
    if !super::live::enabled() { return; }
    let _k = crate::zone_alloc::kept();
    let mut s = state().lock().unwrap();
    let bitmap = Bitmap::new(&s.banks);
    unsafe { *((block + TLS_ENTERED as u64) as *mut u64) = bitmap.table.as_ptr() as u64; }
    s.threads.insert(block, Thread { bitmap, test: None });
}

fn collect(s: &mut State, thread: &Thread, clear: bool) {
    match &thread.test {
        Some(t) if s.tests.get(t.index).and_then(Option::as_ref).is_some_and(|c| Arc::ptr_eq(c, t)) =>
            thread.bitmap.read(&mut t.slots.lock().unwrap(), clear),
        _ => thread.bitmap.read(&mut s.unattributed, clear),
    }
}

/// The thread's block moved (jit.rs window_tls_block): its record follows it.
pub(crate) fn moved(old: u64, new: u64) {
    if !super::live::enabled() { return; }
    let _k = crate::zone_alloc::kept();
    let mut s = state().lock().unwrap();
    if let Some(thread) = s.threads.remove(&old) { s.threads.insert(new, thread); }
}

pub(crate) fn exit(block: u64) {
    if !super::live::enabled() { return; }
    let _k = crate::zone_alloc::kept();
    let mut s = state().lock().unwrap();
    if let Some(thread) = s.threads.remove(&block) {
        collect(&mut s, &thread, false);
        if let Some(t) = thread.test { t.active.fetch_sub(1, Release); }
    }
}

/// The harness starts recording before catch_unwind and seals it after the outcome is captured.
pub(super) extern "C" fn test(index: usize, done: u32) {
    crate::jit::host_boundary("test reach", || {
        let _k = crate::zone_alloc::kept();
        let block = crate::jit::tls_block();
        let mut s = state().lock().unwrap();
        if done == 0 {
            let thread = s.threads.remove(&block).unwrap();
            collect(&mut s, &thread, true);
            if let Some(t) = thread.test { t.active.fetch_sub(1, Release); }
            let t = s.tests[index].get_or_insert_with(|| Arc::new(Capture {
                index, complete: AtomicBool::new(false), invalid: AtomicBool::new(false), active: AtomicUsize::new(0), slots: Mutex::new(HashSet::new()),
            })).clone();
            if t.active.load(Acquire) == 0 {
                t.slots.lock().unwrap().clear();
                t.invalid.store(false, Relaxed);
            }
            t.complete.store(false, Relaxed);
            t.active.fetch_add(1, Relaxed);
            s.threads.insert(block, Thread { bitmap: thread.bitmap, test: Some(t) });
        } else if let Some(t) = &s.threads[&block].test {
            t.complete.store(done == 1, Release);
        }
    });
}

/// Faulting children invalidate their parent's reach even if the test catches the join error.
pub(crate) fn fault() {
    let block = crate::jit::tls_block_if_any();
    if let Some(t) = state().lock().unwrap().threads.get(&block).and_then(|t| t.test.as_ref()) {
        t.invalid.store(true, Release);
    }
}

/// A spawn owns a reservation until adoption or failure, so a delayed child is never missed.
pub(crate) extern "C" fn inherit() -> usize {
    if !super::live::enabled() { return 0; }
    crate::jit::host_boundary("test inherit", || {
        let s = state().lock().unwrap();
        let t = s.threads.get(&crate::jit::tls_block_if_any()).and_then(|t| t.test.clone());
        match t {
            Some(t) => { t.active.fetch_add(1, Relaxed); Arc::into_raw(t) as usize }
            None => 0,
        }
    })
}

pub(crate) extern "C" fn adopt(token: usize) {
    if token == 0 { return; }
    crate::jit::host_boundary("test adopt", || {
        let _k = crate::zone_alloc::kept();
        let block = crate::jit::tls_block();
        let mut s = state().lock().unwrap();
        let mut thread = s.threads.remove(&block).unwrap();
        collect(&mut s, &thread, true);
        if let Some(t) = thread.test { t.active.fetch_sub(1, Release); }
        thread.test = Some(unsafe { Arc::from_raw(token as *const Capture) });
        s.threads.insert(block, thread);
    });
}

pub(crate) extern "C" fn cancel(token: usize) {
    if token == 0 { return; }
    let t = unsafe { Arc::from_raw(token as *const Capture) };
    t.active.fetch_sub(1, Release);
}

pub(super) struct Reach {
    defs: HashSet<DefId>,
    pub unknown: bool,
    /// it started a process (an app it drives): what that process runs is reach no capture here sees
    pub spawned: bool,
}

impl Reach {
    pub(super) fn affected(&self, changed: &[DefId]) -> bool { changed.iter().any(|d| self.defs.contains(d)) }
    pub(super) fn reaches(&self, d: DefId) -> bool { self.defs.contains(&d) }
}

/// std's process spawn ran (every `Command` spawn ends in its `spawn_now`).
fn spawned(u: &Unit, defs: &HashSet<DefId>) -> bool {
    defs.iter().any(|&d| {
        let def = u.prog.def(d);
        u.prog.syms.str(def.name) == "spawn_now" && u.prog.syms.str(u.prog.crates[def.krate as usize].name) == "std"
    })
}

/// Generic instances and closure bodies belong to the edited definition, not the instance.
fn definition(u: &Unit, mut id: u32) -> Option<DefId> {
    loop {
        if id as usize >= u.fns.len() { return None; }
        let mut d = match &u.fns[id as usize].key {
            FnKey::Inst(d, _) => *d,
            FnKey::Closure(_, _, parent) | FnKey::Sandbox(parent) | FnKey::Plain(parent) => { id = *parent; continue; }
            FnKey::Shim(s) => match s.kind {
                relax_types::instance::ShimKind::ClosureOnce { closure, .. } => closure,
                _ => s.kind.def_id(),
            },
            FnKey::Glue(..) => return None,
        };
        while u.prog.def(d).kind == DefKind::Closure { d = u.prog.def(d).parent; }
        return Some(d);
    }
}

fn expand(u: &Unit, s: &State, slots: &HashSet<u32>) -> HashSet<DefId> {
    let mut seen = HashSet::new();
    let mut defs = HashSet::new();
    let mut todo: Vec<u32> = slots.iter().copied().collect();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) { continue; }
        if let Some(d) = definition(u, id) { defs.insert(d); }
        if let Some(deps) = s.inlined.get(&id) { todo.extend(deps); }
    }
    defs
}

/// Read live worker marks atomically; unfinished roots or children keep reach unknown.
pub(super) fn snapshot(u: &Unit) -> (Vec<Reach>, HashSet<DefId>) {
    let _k = crate::zone_alloc::kept();
    let mut s = state().lock().unwrap();
    let threads = std::mem::take(&mut s.threads);
    for thread in threads.values() { collect(&mut s, thread, false); }
    s.threads = threads;
    let tests = s.tests.iter().map(|t| match t {
        Some(t) => {
            let defs = expand(u, &s, &t.slots.lock().unwrap());
            Reach { spawned: spawned(u, &defs), defs, unknown: !t.complete.load(Acquire) || t.invalid.load(Acquire) || t.active.load(Acquire) != 0 }
        }
        None => Reach { defs: HashSet::new(), unknown: true, spawned: false },
    }).collect();
    (tests, expand(u, &s, &s.unattributed))
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&STATE, OnceLock::new());
}

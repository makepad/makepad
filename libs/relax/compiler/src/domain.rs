//! Domain-local statics (local/agent_state/fast-rust/android/DESIGN-domains.md §2).
//!
//! Several apps ("domains") may run in one process on the same compiled code, so each needs its
//! own copy of every static. A domain symbol (`SymClass::Domain`: a static, a writable block
//! lowering placed) therefore has an OFFSET in the domain static block, not an address, and
//! code computes `domain base + offset`: the base is slot 0 of the running thread's
//! thread-local block (jit.rs `tls_alloc`), read by the back ends' `dom_addr` next to the block
//! pointer itself.
//!
//! - The compile domain (domain 0) is the unit's own writable region: statics are laid out in
//!   it as they are first named, initializers and constant evaluation run in it, and a
//!   thread that is in no other domain uses it. Nothing about a single-domain run differs from
//!   absolute addresses but the base load.
//! - Another domain's block is a same-sized range of its own, reserved once and never moved.
//!   Its bytes are the INITIAL images (`Syms::initial_data`, the snapshot taken right after the
//!   initializer, never the running state) at the same offsets as in domain 0; the pointer words
//!   in them that point into the block (statics naming statics, cycles, promoted writable
//!   data) are rebased, words that point into shared read-only data stay. The attribution of
//!   words and the image bytes are the sandbox's (`sandbox::provenance_words`, `initial_bytes`):
//!   the same "one data image, pointers relocated per copy" machinery, a second client.
//! - A static laid out after a domain exists is copied into every live block (`materialize`).
//! - Threads: a thread JIT code starts inherits its parent's base (std's thread start calls
//!   `relax_domain_base` / `relax_domain_enter`); `call_jit` runs in the unit's domain 0; a
//!   thread that native code starts gets the runtime unit's compile domain from `rt_tls_block`.
//!
//! A constant's value is shared read-only data, but one that points at a static (`const C:
//! &AtomicU32 = &S`), or at other per-domain data, directly or through data it points to, would
//! hand every domain the compile domain's copy. `rehome_const` moves such a value (and each data
//! object on the way that reaches the block) into a writable per-domain object: every domain then
//! has its own copy with its pointers rebased, and code reaches it through its symbol like a
//! static. A scalar pointer constant needs nothing: lowering emits its target's symbol.

use crate::jit::Unit;
use relax_backend::rir::Sym;
use crate::sandbox::{initial_bytes, provenance_words};
use crate::syms::SymKind;
use std::collections::HashSet;
use relax_session::sync::Mutex;

/// The address of constant data object `addr` as every domain must see it: unchanged when its
/// image (and the data it points to) holds no address inside the compile domain's block; else a
/// writable per-domain block (`Unit::rw_sym`: SymClass::Domain, never folded, so code reads it
/// through its symbol), its pointer words patched to the re-homed children, registered as initial data so
/// live and later domains get it rebased (`materialize`, `create`). Bottom-up; `seen` keeps shared
/// children (and cycles) to one copy.
pub(crate) fn rehome_const(u: &Unit, addr: u64) -> u64 {
    let mut seen = std::collections::HashMap::new();
    rehome(u, addr, &mut seen, false)
}

/// The per-domain copy of constant data object `addr` whatever its words hold (its children
/// re-homed as `rehome_const` does): what code compiled against such a copy addresses -- a
/// domain symbol -- when it is linked from the disk cache (cache.rs link_symbol tag 10).
/// `addr` itself when it cannot be copied (not the start of a read-only object).
pub(crate) fn const_copy(u: &Unit, addr: u64) -> u64 {
    let mut seen = std::collections::HashMap::new();
    rehome(u, addr, &mut seen, true)
}

/// Captures static `sym`'s initial image (just evaluated into its storage) and the image of
/// every anonymous mutable block its words point into, then (the caller) materializes it. A
/// static initializer that runs as code fills its promoted temporaries (`&CONST` /
/// `&[..]` borrows lowered as a per-domain block written by the initializer, lower.rs
/// promote_rvalue) in the compile domain only; without their images every other domain's copy
/// of the static pointed at zeroed blocks (Sandbox's race kit read each event payload's
/// FieldDefs as zeros: no names). `rehome` does the same for constants.
pub(crate) fn capture_static(u: &Unit, sym: Sym) {
    crate::sandbox::capture_initial_data(u, sym);
    let Ok(bytes) = initial_bytes(u, sym.id) else { return };
    let mut seen = HashSet::new();
    for w in bytes.chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().unwrap())) {
        if let Some(rw) = u.syms.rw_sym_at(w) {
            capture_rw(u, rw.id, &mut seen);
        }
    }
}

/// Gives anonymous mutable block `id` (made by constant evaluation: a promoted temporary) and
/// every such block its image points into an initial image, and copies them into the live
/// domains: they have none until something names them.
fn capture_rw(u: &Unit, id: u32, seen: &mut HashSet<u32>) {
    if !seen.insert(id) {
        return;
    }
    let sym = Sym::new(id);
    crate::sandbox::capture_initial_data(u, sym);
    if let Ok(bytes) = initial_bytes(u, id) {
        for w in bytes.chunks_exact(8).map(|w| u64::from_le_bytes(w.try_into().unwrap())) {
            if let Some(rw) = u.syms.rw_sym_at(w) {
                capture_rw(u, rw.id, seen);
            }
        }
    }
    materialize(u, sym);
}

fn rehome(u: &Unit, addr: u64, seen: &mut std::collections::HashMap<u64, u64>, force: bool) -> u64 {
    if let Some(&a) = seen.get(&addr) {
        return a;
    }
    let (lo, hi) = (u.syms.dom0(), u.syms.dom0() + BLOCK_SIZE as u64);
    let Some((start, len, align, mutable)) = u.syms.object_of(addr) else { return addr };
    if mutable || start != addr || (lo..hi).contains(&start) {
        return addr;
    }
    // a cycle back to this object keeps the shared address until the copy exists (patched below)
    seen.insert(addr, addr);
    let mut bytes = unsafe { std::slice::from_raw_parts(start as *const u8, len as usize).to_vec() };
    // only the words that are pointers by the object's type: an integer that happens to equal
    // an address inside the block (a char pair of unicode-segmentation's word_cat_table did, in
    // some runs) is not a reference to a static, and copying such a constant into a per-domain
    // block took it off the shared data for nothing (and the disk cache named the copy as a
    // fresh, zeroed block: the warm run read a length of 0). An object whose kind cannot tell
    // is scanned word by word, as before.
    let pw = u.ptr_bytes() as usize;
    let words: Vec<usize> = match crate::syms::pointer_offsets(u, start) {
        Some(offs) => offs.into_iter().map(|o| o as usize).filter(|&o| o + pw <= bytes.len()).collect(),
        None => (0..bytes.len() / pw).map(|i| i * pw).collect(),
    };
    let mut reaches = false;
    for &k in &words {
        let w = crate::syms::word_in(u, &bytes, k);
        if (lo..hi).contains(&w) {
            // a static (materialized when laid out) or an anonymous mutable block: one constant
            // evaluation made (a promoted temporary) has no initial image yet -- every domain needs it
            if let Some(rw) = u.syms.rw_sym_at(w) {
                capture_rw(u, rw.id, &mut HashSet::new());
            }
            reaches = true;
            continue;
        }
        let Some((cs, _, _, cmut)) = u.syms.object_of(w) else { continue };
        if cmut || cs == start {
            continue;
        }
        let moved = rehome(u, cs, seen, false);
        if moved != cs {
            bytes[k..k + pw].copy_from_slice(&u.ptr_word(moved + (w - cs)));
            reaches = true;
        }
    }
    if !reaches && !force {
        seen.insert(addr, addr);
        return addr;
    }
    // an anonymous mutable block: per domain (SymClass::Domain), never folded
    let sym = u.rw_sym(len.max(1), align.max(1), match u.syms.kind_at(start) { crate::syms::DataKind::Typed(t) => Some(t), _ => None });
    let new = u.syms.addr(sym);
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), new as *mut u8, bytes.len()) };
    // a word pointing into the object itself (a self-referential constant) points at the copy
    for &k in &words {
        let w = crate::syms::word_in(u, &bytes, k);
        if (start..start + len).contains(&w) {
            unsafe { crate::syms::write_ptr(u, new + k as u64, new + (w - start)) };
        }
    }
    seen.insert(addr, new);
    crate::zone_alloc::kept_with(|| u.syms.const_copies.lock().unwrap_or_else(|e| e.into_inner()).insert(sym.id, start));
    crate::sandbox::capture_initial_data(u, sym);
    materialize(u, sym);
    new
}

/// Blocks of the domains other than the compile domain, by base.
static BLOCKS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Bytes of a domain block (the unit's writable region; a block is address space until touched).
pub const BLOCK_SIZE: usize = crate::jit::UNIT_RW_SIZE as usize;

/// The domain a thread nothing assigned one to starts in (one native code started and calls
/// into JIT code, e.g. a platform callback): the compile domain of the runtime's unit (the
/// program's, `call_jit` / `set_runtime_unit`; a process can hold other units: the macro world's,
/// test units). 0 before any unit ran code.
pub fn default_base() -> u64 {
    let u = unsafe { crate::jit::UNIT };
    if u.is_null() { 0 } else { unsafe { (&*u).syms.dom0() } }
}

/// The calling thread's domain block (slot 0 of its thread-local block).
pub fn current() -> u64 {
    unsafe { *(crate::jit::tls_block() as *const u64) }
}

/// Puts the calling thread in the domain whose block is `base`; returns the one it was in.
pub fn enter(base: u64) -> u64 {
    let slot = crate::jit::tls_block() as *mut u64;
    register_current(base);
    let heap = domains().iter().find(|d| d.info.base == base && base != 0)
        .and_then(|d| d.heap.as_deref()).map_or(std::ptr::null(), |h| h as *const _);
    unsafe {
        crate::zone_alloc::domain::enter(heap);
        std::mem::replace(&mut *slot, base)
    }
}

/// For Relax std (`__relax_domain_base`): the base a thread it starts inherits.
pub extern "C" fn relax_domain_base() -> u64 {
    current()
}

/// For Relax std (`std::relax::domain`): the id of the app domain the calling thread runs in
/// (0: the compile domain, or a non-HAL program running directly).
pub extern "C" fn relax_domain_id() -> u32 {
    current_app_domain().unwrap_or(0)
}

/// For Relax std (`__relax_domain_enter`): the first thing a started thread does.
pub extern "C" fn relax_domain_enter(base: u64, token: u64) {
    let _kept = crate::zone_alloc::kept();
    unsafe {
        let t = crate::domain_threads::data(crate::jit::tls_block());
        if token != 0 { t.registrations.push((base, token)); }
    }
    enter(base);
}

fn register_current(base: u64) {
    if base == 0 { return; }
    let _kept = crate::zone_alloc::kept();
    let t = unsafe { crate::domain_threads::data(crate::jit::tls_block()) };
    if t.registrations.iter().any(|r| r.0 == base) { return; }
    let token = relax_domain_reserve(base);
    if token != 0 { t.registrations.push((base, token)); }
}

static NEXT_THREAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Reserve before OS thread creation: main may return before a child reaches its entry.
pub extern "C" fn relax_domain_reserve(base: u64) -> u64 {
    let token = NEXT_THREAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some(d) = domains().iter_mut().find(|d| d.info.base == base && base != 0) {
        d.threads.push((token, format!("thread-{token}")));
        return token;
    }
    0
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Callback {
    id: DomainId,
    base: u64,
    started: std::time::Instant,
}

pub(crate) fn callback() -> Option<Callback> {
    let base = current();
    domains().iter().find(|d| d.info.base == base && base != 0).map(|d| Callback { id: d.info.id, base, started: d.started })
}

impl Callback {
    pub(crate) fn domain(self) -> DomainId { self.id }

    pub(crate) fn running(self) -> bool {
        domains().iter().any(|d| d.info.base == self.base && d.started == self.started && !d.cancel && matches!(d.info.state, DomainState::Running))
    }

    /// A host worker may call a domain's wake without becoming a permanent app thread. Reserve
    /// under the teardown lock, reject an outgoing incarnation, and retire its TLS after the call.
    pub(crate) fn call<R>(self, f: impl FnOnce() -> R) -> Option<R> {
        self.invoke(false, f)
    }

    /// Close a producer already owned by this incarnation, including during cooperative stop.
    pub(crate) fn cleanup<R>(self, f: impl FnOnce() -> R) -> Option<R> {
        self.invoke(true, f)
    }

    fn invoke<R>(self, cleanup: bool, f: impl FnOnce() -> R) -> Option<R> {
        let token = {
            let mut ds = domains();
            let d = ds.iter_mut().find(|d| d.info.base == self.base && d.started == self.started)?;
            if d.info.id == 0 { drop(ds); return Some(f()); }
            if d.reclaiming || !cleanup && (d.cancel || !matches!(d.info.state, DomainState::Running)) { return None; }
            let token = NEXT_THREAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            d.threads.push((token, "host callback".into()));
            token
        };
        Some(crate::jit::domain_callback(self.base, token, f))
    }
}

pub extern "C" fn relax_domain_name(name: *const u8) {
    let base = current();
    let t = unsafe { crate::domain_threads::data(crate::jit::tls_block()) };
    let Some((_, token)) = t.registrations.iter().find(|r| r.0 == base) else { return };
    let name = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned();
    if let Some(d) = domains().iter_mut().find(|d| d.info.base == base) {
        if let Some(entry) = d.threads.iter_mut().find(|e| e.0 == *token) { entry.1 = name; }
    }
}

/// Called after the TLS block and every Relax std TLS destructor are finished (also on a
/// failed OS spawn, while the parent still holds its own registration).
pub extern "C" fn thread_finished(base: u64, token: u64) {
    if token == 0 { return; }
    let id = {
        let mut ds = domains();
        let Some(d) = ds.iter_mut().find(|d| d.info.base == base) else { return };
        d.threads.retain(|e| e.0 != token);
        d.info.id
    };
    teardown(id);
    CHANGED.notify_all();
}

/// The offset in a domain block of what `s` names, when its storage is per domain: a static, a
/// writable block, a writable data object (promoted inside an initializer).
fn offset_of(u: &Unit, s: Sym) -> Option<u64> {
    match u.syms.kind(s.id)? {
        SymKind::Static(_) | SymKind::Rw(_) => Some(u.syms.value(s)),
        SymKind::Data(i) => {
            let (start, _, _, mutable) = u.syms.obj_info(i)?;
            mutable.then(|| (start - u.syms.dom0()).wrapping_add(s.off as u64))
        }
        _ => None,
    }
}

/// Writes the initial image of `sym` (and of the writable data it points to) into the block at
/// `base`, its in-block pointers rebased. `seen`: what this call already did.
fn materialize_in(u: &Unit, base: u64, sym: u32, seen: &mut HashSet<u32>) {
    if !seen.insert(sym) {
        return;
    }
    let Some(own) = offset_of(u, Sym::new(sym)) else { return };
    // no declared initializer: a block lowering fills by code when it runs stays zero
    let Ok(bytes) = initial_bytes(u, sym) else { return };
    let at = base + own;
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len()) };
    for (o, t) in provenance_words(u, &bytes) {
        if let Some(to) = offset_of(u, t) {
            unsafe { crate::syms::write_ptr(u, at + o as u64, base + to) };
            if !matches!(u.syms.kind(t.id), Some(SymKind::Static(_) | SymKind::Rw(_))) {
                materialize_in(u, base, t.id, seen);
            }
        }
    }
}

/// Makes a new domain's block: reserved once (it never moves), holding the initial image of
/// every static laid out so far. Returns its base.
pub fn create(u: &Unit) -> u64 {
    // a wasm32 program's statics are in its window (web32.rs)
    let base = if crate::web32::on() { crate::web32::alloc(BLOCK_SIZE) } else { crate::os::mem::alloc_rw(BLOCK_SIZE) };
    let ids: Vec<u32> = u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect();
    let mut seen = HashSet::new();
    for id in ids {
        materialize_in(u, base, id, &mut seen);
    }
    {
        let _kept = crate::zone_alloc::kept();
        BLOCKS.lock().unwrap_or_else(|e| e.into_inner()).push(base);
    }
    base
}

/// Gives a domain's block back (every thread of the domain is gone).
pub fn destroy(base: u64) {
    forget_tls_keys(base);
    BLOCKS.lock().unwrap_or_else(|e| e.into_inner()).retain(|b| *b != base);
    if crate::web32::on() { crate::web32::free(base, BLOCK_SIZE) } else { crate::os::mem::free(base, BLOCK_SIZE) }
}

/// A static was just laid out (jit.rs `static_addr`, after its initial image was captured): copy
/// it into the live blocks. No other domain: nothing to do.
pub(crate) fn materialize(u: &Unit, sym: Sym) {
    let blocks = BLOCKS.lock().unwrap_or_else(|e| e.into_inner());
    for &base in blocks.iter() {
        materialize_in(u, base, sym.id, &mut HashSet::new());
    }
}

/// The live blocks other than domain 0 (tests, inspect).
pub fn blocks() -> Vec<u64> {
    BLOCKS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

// ---------------------------------------------------------------------------------------------
// Apps as domains: load an app's crate graph into the running unit, run its main in a domain.
// ---------------------------------------------------------------------------------------------

/// A domain's id (the host's session id when a host names it: `AppSpec::id`). 0 is the app
/// `makepad run` booted.
pub type DomainId = u32;

/// Where a domain is.
#[derive(Clone, Debug, PartialEq)]
pub enum DomainState {
    /// loading: the phase it is in
    Compiling(String),
    Running,
    /// main panicked (or faulted): the message
    Crashed(String),
    /// main returned (the exit code of `fn main() -> ExitCode`)
    Exited(i32),
    /// `stop` marked it
    Stopped,
    /// Cooperative shutdown timed out. Its entire domain remains owned, even if it later ends.
    Leaked { threads: usize },
}

/// Runs a closure on the calling thread, with whatever the host sets up around it (its
/// capabilities, the launch a HAL session is opened with): the host's hook around the app's `main`.
pub type Around = std::sync::Arc<dyn Fn(&mut dyn FnMut()) + Send + Sync>;

/// The app to start as a domain.
#[derive(Clone, Default)]
pub struct AppSpec {
    /// the cargo package (as `makepad run <package>`)
    pub package: String,
    /// cargo flags (features, ...) as `makepad run <package> <flags>`
    pub cargo_flags: Vec<String>,
    pub units_json: String,
    /// the app's arguments
    pub args: Vec<String>,
    /// A boot invocation's exact argv, including argv[0] and non-UTF8 arguments.
    pub argv: Option<Vec<Vec<u8>>>,
    /// the domain this one is embedded in
    pub parent: Option<DomainId>,
    /// the id the host allocated (its session id); None: the next free one
    pub id: Option<DomainId>,
    /// the host's hook around the app's whole run, on the domain's thread (host-native code,
    /// never JIT code; nothing app-specific)
    pub around: Option<Around>,
}

/// One domain, as `list` / `state` report it.
#[derive(Clone, Debug)]
pub struct DomainInfo {
    pub id: DomainId,
    pub package: String,
    pub parent: Option<DomainId>,
    pub state: DomainState,
    /// the base of its static block
    pub base: u64,
    /// crates of its package graph / shared with the unit already / loaded for it
    pub crates: usize,
    pub reused: usize,
    pub loaded: usize,
    /// the crates loaded for it
    pub new_crates: Vec<String>,
    /// ms from the call of `start` to main being called, split: package graph + build scripts,
    /// loading the new crates, the rest (find main, block, thread)
    pub resolve_ms: f64,
    pub load_ms: f64,
    pub spawn_ms: f64,
    /// ms from `start` to the domain's first frame (None: not yet)
    pub first_frame_ms: Option<f64>,
    pub threads: usize,
    pub thread_names: Vec<String>,
    pub arena_in_use: usize,
    pub arena_committed: usize,
    /// bytes touched in small chunks / in live slots / in large blocks (the arena's detail)
    pub arena_detail: (usize, usize, usize),
    pub static_committed: u64,
}

struct Domain {
    info: DomainInfo,
    started: std::time::Instant,
    /// the app's argv (argv[0] first); empty: the process's (domain 0)
    args: Vec<Vec<u8>>,
    /// the domain's main thread has returned (its block can go)
    thread_done: bool,
    compiling: bool,
    reclaiming: bool,
    cancel: bool,
    threads: Vec<(u64, String)>,
    heap: Option<Box<crate::zone_alloc::domain::DomainHeap>>,
}

/// The argv of the domain whose block is at `base` (None: the process's own).
pub(crate) fn args_of(base: u64) -> Option<Vec<Vec<u8>>> {
    domains().iter().find(|d| d.info.base == base && !d.args.is_empty()).map(|d| d.args.clone())
}

static DOMAINS: Mutex<Vec<Domain>> = Mutex::new(Vec::new());
static CHANGED: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
type Observer = std::sync::Arc<dyn Fn(DomainId, &DomainState) + Send + Sync>;
static OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

/// The registry lock, held in the kept phase: the registry is process state that outlives the
/// settle swap (zone_swap destroys the compiler arena generation), so whatever a mutator
/// allocates while holding it (records, names, strings) lands in the kept heap, whichever
/// phase the calling thread is in. Values built before the lock is taken are copied in with
/// `kept_clone`.
struct Registry {
    guard: relax_session::sync::MutexGuard<'static, Vec<Domain>>,
    _kept: crate::zone_alloc::PhaseGuard,
}
impl std::ops::Deref for Registry {
    type Target = Vec<Domain>;
    fn deref(&self) -> &Vec<Domain> { &self.guard }
}
impl std::ops::DerefMut for Registry {
    fn deref_mut(&mut self) -> &mut Vec<Domain> { &mut self.guard }
}
impl Registry {
    fn wait(self) -> Self {
        let Registry { guard, _kept } = self;
        Registry { guard: CHANGED.wait(guard).unwrap_or_else(|e| e.into_inner()), _kept }
    }
    fn wait_timeout(self, left: std::time::Duration) -> Self {
        let Registry { guard, _kept } = self;
        Registry { guard: CHANGED.wait_timeout(guard, left).unwrap_or_else(|e| e.into_inner()).0, _kept }
    }
}
fn domains() -> Registry {
    let kept = crate::zone_alloc::kept();
    Registry { guard: DOMAINS.lock().unwrap_or_else(|e| e.into_inner()), _kept: kept }
}

/// The host's callback for state changes (compiling phases, running, crashed, exited).
pub fn set_observer(f: impl Fn(DomainId, &DomainState) + Send + Sync + 'static) {
    *OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(f));
}

fn set_state(id: DomainId, st: DomainState) {
    {
        let mut d = domains();
        if let Some(x) = d.iter_mut().find(|x| x.info.id == id) {
            if matches!(x.info.state, DomainState::Leaked { .. }) { return; }
            if x.cancel && matches!(st, DomainState::Compiling(_) | DomainState::Running) { return; }
            x.info.state = st.clone();
        }
    }
    notify_state(id, &st);
}

/// A non-HAL `makepad run` booted `package` in the unit's own domain: domain 0.
pub(crate) fn register_boot(package: &str, base: u64) {
    domains().push(Domain {
        info: DomainInfo { id: 0, package: package.to_string(), parent: None, state: DomainState::Running, base, crates: 0, reused: 0, loaded: 0, new_crates: Vec::new(), resolve_ms: 0.0, load_ms: 0.0, spawn_ms: 0.0, first_frame_ms: None, threads: 0, thread_names: Vec::new(), arena_in_use: 0, arena_committed: 0, arena_detail: (0, 0, 0), static_committed: 0 },
        started: std::time::Instant::now(),
        args: Vec::new(),
        thread_done: false,
        compiling: false, reclaiming: false, cancel: false, threads: Vec::new(), heap: None,
    });
    let token = relax_domain_reserve(base);
    relax_domain_enter(base, token);
}

/// The domain running on the calling thread presented its first frame (jit.rs `relax_first_present`).
pub(crate) fn first_present() {
    let base = current();
    {
        let mut d = domains();
        if let Some(x) = d.iter_mut().find(|x| x.info.base == base && x.info.first_frame_ms.is_none()) {
            x.info.first_frame_ms = Some(x.started.elapsed().as_secs_f64() * 1e3);
            crate::run::domain_present();
        }
    }
    CHANGED.notify_all();
}

fn snapshot(d: &Domain) -> DomainInfo {
    let mut i = d.info.clone();
    i.threads = d.threads.len();
    i.thread_names = d.threads.iter().map(|t| t.1.clone()).collect();
    (i.arena_in_use, i.arena_committed) = d.heap.as_ref().map_or((0, 0), |h| h.stats());
    i.arena_detail = d.heap.as_ref().map_or((0, 0, 0), |h| { let x = h.detail(); (x.0, x.1, x.2) });
    i
}
fn with_block_memory(mut i: DomainInfo) -> DomainInfo {
    // mincore may walk a large reservation. It needs no domain lock and returns failure if a
    // concurrent teardown removed the mapping; memory inspection is an independent snapshot.
    i.static_committed = if i.base == 0 { 0 } else { block_committed(i.base) };
    i
}
pub fn list() -> Vec<DomainInfo> {
    let snapshots: Vec<_> = domains().iter().map(snapshot).collect();
    snapshots.into_iter().map(with_block_memory).collect()
}

/// The app domain's heap: (requested live bytes, committed bytes), without the block walk of
/// `info` (the flight recorder's 1-s counter).
pub fn app_heap_stats() -> Option<(usize, usize)> {
    let id = crate::run::app_domain()?;
    domains().iter().find(|d| d.info.id == id).and_then(|d| d.heap.as_ref().map(|h| h.stats()))
}

pub fn state(id: DomainId) -> Option<DomainState> {
    domains().iter().find(|d| d.info.id == id).map(|d| d.info.state.clone())
}

pub fn info(id: DomainId) -> Option<DomainInfo> {
    let snapshot = domains().iter().find(|d| d.info.id == id).map(snapshot);
    snapshot.map(with_block_memory)
}

/// Waits (up to `timeout`) until domain `id` presented its first frame, or ended: its first frame
/// time in ms since `start`.
pub fn wait_first_frame(id: DomainId, timeout: std::time::Duration) -> Option<f64> {
    let end = std::time::Instant::now() + timeout;
    let mut d = domains();
    loop {
        match d.iter().find(|x| x.info.id == id) {
            Some(x) if x.info.first_frame_ms.is_some() => return x.info.first_frame_ms,
            Some(x) if matches!(x.info.state, DomainState::Crashed(_) | DomainState::Exited(_) | DomainState::Stopped | DomainState::Leaked { .. }) => return None,
            None => return None,
            _ => {}
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        d = d.wait_timeout(left);
    }
}

/// How a domain is to end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stop {
    /// through the app's own exit path (the host's window close): not Relax's to do
    Quit,
    /// now
    Kill,
}

/// The HAL already delivered quit (or freed the session). Never terminate a thread: retain
/// everything on timeout, because a late callback can still use its statics and leaked boxes.
pub fn stop(id: DomainId, how: Stop) -> Result<(), String> {
    let ms = std::env::var("MAKEPAD_RUN_DOMAIN_JOIN_MS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(5000);
    stop_for(id, std::time::Duration::from_millis(if how == Stop::Kill { ms / 5 } else { ms }))
}

/// Close admission to app callbacks before the host cancels its producers and joins threads.
pub fn begin_stop(id: DomainId) {
    if let Some(d) = domains().iter_mut().find(|d| d.info.id == id && id != 0) { d.cancel = true; }
    CHANGED.notify_all();
}

fn stop_for(id: DomainId, timeout: std::time::Duration) -> Result<(), String> {
    if id == 0 { return Err("domain 0 is the process's own app".into()); }
    let deadline = std::time::Instant::now() + timeout;
    let mut ds = domains();
    loop {
        let d = ds.iter_mut().find(|d| d.info.id == id).ok_or_else(|| format!("no domain {id}"))?;
        d.cancel = true;
        if matches!(d.info.state, DomainState::Leaked { .. }) { return Ok(()); }
        if !d.compiling && !d.reclaiming && d.threads.is_empty() {
            if d.thread_done && d.info.base == 0 { return Ok(()); }
            drop(ds);
            teardown(id);
            // The last thread may have started reclamation between unlocking and teardown.
            // Recheck its completion instead of returning while it still owns the mapping.
            ds = domains();
            continue;
        }
        if d.reclaiming {
            ds = ds.wait();
            continue;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            if !d.threads.is_empty() {
                let n = d.threads.len();
                let names = d.threads.iter().map(|t| t.1.as_str()).collect::<Vec<_>>().join(", ");
                // Publish while holding the same lock as the last-thread teardown decision.
                d.info.state = DomainState::Leaked { threads: n };
                drop(ds);
                eprintln!("makepad-run domain: {id}: leaked {n} threads: {names}");
                notify_state(id, &DomainState::Leaked { threads: n });
            } else {
                // Compilation owns its temporary state until the next cancellation boundary.
                drop(ds);
                set_state(id, DomainState::Stopped);
            }
            return Ok(());
        }
        ds = ds.wait_timeout(left);
    }
}

static FREED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn freed_by_teardown() -> u64 { FREED.load(std::sync::atomic::Ordering::Relaxed) }

fn notify_state(id: DomainId, state: &DomainState) {
    CHANGED.notify_all();
    // Closing a parent may join its children, whose teardown also notifies this observer.
    // No registry lock may span that callback.
    let observer = OBSERVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(f) = observer { f(id, state); }
}

fn teardown(id: DomainId) {
    let mut ds = domains();
    let Some(d) = ds.iter_mut().find(|d| d.info.id == id) else { return };
    if id == 0 || d.compiling || !d.threads.is_empty() || matches!(d.info.state, DomainState::Leaked { .. }) { return; }
    if d.reclaiming || d.thread_done && d.info.base == 0 { return; }
    d.reclaiming = true;
    let base = std::mem::take(&mut d.info.base);
    let heap = d.heap.take();
    let args = std::mem::take(&mut d.args);
    let threads = std::mem::take(&mut d.threads);
    let crates = std::mem::take(&mut d.info.new_crates);
    drop(ds);
    // Unmapping and dropping tables can take time. Registry readers (including the UI) must
    // not wait behind those operations, and stop must not report completion ahead of them.
    let mut freed = 0;
    if base != 0 {
        freed += block_committed(base);
        destroy(base);
    }
    if let Some(heap) = heap {
        let (base, size) = heap.range();
        crate::inspect::heap::forget_range(base, size);
        freed += (heap.stats().1 + heap.metadata_bytes()) as u64;
        drop(heap);
    }
    drop((args, threads, crates));
    // The process heap keeps what the domain's threads freed in their malloc arenas (free but
    // held, growing every cycle): hand it back before the stop completes.
    crate::memstat::trim();
    let mut ds = domains();
    let d = ds.iter_mut().find(|d| d.info.id == id).expect("reclaiming domain");
    d.thread_done = true;
    if matches!(d.info.state, DomainState::Compiling(_) | DomainState::Running) || d.cancel && !matches!(d.info.state, DomainState::Exited(_) | DomainState::Crashed(_)) {
        d.info.state = DomainState::Stopped;
    }
    let state = d.info.state.clone();
    FREED.fetch_add(freed, std::sync::atomic::Ordering::Relaxed);
    drop(ds);
    notify_state(id, &state);
    // A replacement must not start until the outgoing run's final observer returned. The
    // observer carries this id; delivering it later could close the replacement's HAL session.
    if let Some(d) = domains().iter_mut().find(|d| d.info.id == id) { d.reclaiming = false; }
    CHANGED.notify_all();
}

pub(crate) fn cancelled(id: DomainId) -> bool { domains().iter().find(|d| d.info.id == id).is_some_and(|d| d.cancel) }
pub(crate) fn compile_phase(id: DomainId, phase: &str) -> Result<(), String> {
    if cancelled(id) { return Err("domain compilation cancelled".into()); }
    set_state(id, DomainState::Compiling(phase.into()));
    if cancelled(id) { return Err("domain compilation cancelled".into()); }
    Ok(())
}

// Keys are domain-local records, not OS keys that survive each restart. Domain 0 also owns
// keys for native callback threads; every ThreadData holds only its own values.
/// Per key: its destructor and whether it is late (domain_threads::key_create).
static TLS_KEYS: Mutex<Vec<(u64, Vec<Option<(usize, bool)>>)>> = Mutex::new(Vec::new());
pub(crate) fn tls_key_create(base: u64, dtor: usize, late: bool) -> u32 {
    let _kept = crate::zone_alloc::kept();
    let mut keys = TLS_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    let i = match keys.iter().position(|k| k.0 == base) {
        Some(i) => i,
        None => { keys.push((base, Vec::new())); keys.len() - 1 }
    };
    let values = &mut keys[i].1;
    let key = values.len() as u32;
    values.push(Some((dtor, late)));
    key
}
pub(crate) fn tls_key_delete(base: u64, key: u32) {
    let mut keys = TLS_KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, values)) = keys.iter_mut().find(|k| k.0 == base) {
        if let Some(k) = values.get_mut(key as usize) { *k = None; }
    }
}
pub(crate) fn tls_destructor(base: u64, key: u32) -> Option<(usize, bool)> {
    TLS_KEYS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|k| k.0 == base)?.1.get(key as usize).copied().flatten()
}
pub(crate) fn forget_tls_keys(base: u64) {
    TLS_KEYS.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| k.0 != base);
}

// ---- crate identity

/// Crates of the unit by identity: what loaded them (the boot, a domain start) recorded.
static LOADED: Mutex<Vec<(u64, u32, String, Vec<String>, String)>> = Mutex::new(Vec::new());

/// What makes two crates of two package graphs the same compiled crate: name, source file, edition,
/// cfg set (features and the build script's cfgs), the env!() environment (build script output
/// included), the proc-macro flag, and, by extern name, the identities of its dependencies.
/// Per crate of `g`, in `order` (every crate after its dependencies).
pub(crate) fn identities(g: &crate::unitgraph::Graph, cfgs: &[crate::cfg::CfgSet], order: &[usize]) -> Vec<u64> {
    use std::hash::{Hash, Hasher};
    let mut out = vec![0u64; g.crates.len()];
    for &ci in order {
        let c = &g.crates[ci];
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&c.name, &c.src_path, c.edition, c.proc_macro).hash(&mut h);
        let mut set = cfgs[ci].set.clone();
        set.sort();
        set.hash(&mut h);
        let mut env = g.env[ci].clone();
        env.sort();
        env.hash(&mut h);
        for (d, ext) in &c.deps {
            (ext, out.get(*d).copied().unwrap_or(0)).hash(&mut h);
        }
        out[ci] = h.finish();
    }
    out
}

pub(crate) fn record_loaded(ident: u64, krate: u32, c: &crate::crates::CrateUnit, cfg: &crate::cfg::CfgSet) {
    // the record is process state (see `Registry`): all of it is built in the kept heap
    let _kept = crate::zone_alloc::kept();
    let mut set = cfg.set.clone();
    set.sort();
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    loaded.retain(|x| x.0 != ident);
    loaded.push((ident, krate, c.name.clone(), set, c.src_path.clone()));
}

/// Why a crate of the app's graph is loaded again when the unit has one of its name: the cfg
/// entries (features) that differ, a different source, or else a dependency (or env) that does.
fn namesake_diff(c: &crate::crates::CrateUnit, cfg: &crate::cfg::CfgSet) -> Option<String> {
    let mut set = cfg.set.clone();
    set.sort();
    let l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let other = l.iter().rev().find(|x| x.2 == c.name)?;
    if other.4 != c.src_path {
        return Some(format!("{}: source {} vs {}", c.name, c.src_path, other.4));
    }
    let plus: Vec<&String> = set.iter().filter(|x| !other.3.contains(x)).collect();
    let minus: Vec<&String> = other.3.iter().filter(|x| !set.contains(x)).collect();
    if plus.is_empty() && minus.is_empty() {
        return Some(format!("{}: same cfg, a dependency or its env differs", c.name));
    }
    let f = |v: Vec<&String>| v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ");
    Some(format!("{}: cfg +[{}] -[{}]", c.name, f(plus), f(minus)))
}

fn lookup_loaded(ident: u64) -> Option<u32> {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.0 == ident).map(|x| x.1)
}

// ---- measurements

/// (resident set, proportional set) of the process in KB. Linux: /proc (statm, smaps_rollup);
/// macOS: the resident size and, as the proportional figure, the physical footprint (task_info,
/// what Activity Monitor shows); 0 where neither exists.
pub fn process_memory_kb() -> (u64, u64) {
    let rss = crate::memstat::resident() / 1024;
    let pss = std::fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Pss:")).and_then(|l| l.split_whitespace().nth(1).and_then(|x| x.parse::<u64>().ok())))
        .or_else(|| crate::memstat::footprint().map(|b| b / 1024))
        .unwrap_or(0);
    (rss, pss)
}

/// Where the process's memory sits, for the stop report: glibc malloc (in use / free but held /
/// mmapped), thread count, and the proportional set by mapping kind (the biggest ones).
pub fn process_breakdown() -> String {
    let mut out = String::new();
    #[cfg(target_os = "linux")]
    {
        #[repr(C)]
        struct MallInfo2 { arena: usize, ordblks: usize, smblks: usize, hblks: usize, hblkhd: usize, usmblks: usize, fsmblks: usize, uordblks: usize, fordblks: usize, keepcost: usize }
        extern "C" { fn mallinfo2() -> MallInfo2; }
        let m = unsafe { mallinfo2() };
        let kb = |x: usize| x / 1024;
        out.push_str(&format!("malloc_kb: arena={} in_use={} free_held={} mmapped={} | ", kb(m.arena), kb(m.uordblks), kb(m.fordblks), kb(m.hblkhd)));
        let (cc, cl, _) = crate::zone_alloc::compiler_stats();
        let (touched, slots, large, _) = crate::zone_alloc::compiler_detail();
        out.push_str(&format!("compiler_zone_kb: committed={} live={} small_touched={} small_slots={} large={} | ", cc / 1024, cl / 1024, touched / 1024, slots / 1024, large / 1024));
        let up = crate::run::running_unit();
        if !up.is_null() {
            let u = unsafe { &*up };
            out.push_str(&format!("unit: fns={} bodies={} consts={} statics={} body_deps={} files={} crates={} | ", u.fns.len(), u.bodies.len(), u.consts.len(), u.statics.len(), u.body_deps.len(), u.prog.files.len(), u.prog.crates.len()));
        }
        let threads = std::fs::read_to_string("/proc/self/status").ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("Threads:")).and_then(|l| l.split_whitespace().nth(1).map(str::to_string)))
            .unwrap_or_default();
        let roll = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
        let field = |n: &str| roll.lines().find(|l| l.starts_with(n)).and_then(|l| l.split_whitespace().nth(1).map(str::to_string)).unwrap_or_default();
        out.push_str(&format!("threads={threads} anon_kb={} file_rss_kb={} | pss_kb/size_kb x count by mapping: ", field("Anonymous:"), field("Rss:").parse::<u64>().unwrap_or(0).saturating_sub(field("Anonymous:").parse::<u64>().unwrap_or(0))));
        if let Ok(smaps) = std::fs::read_to_string("/proc/self/smaps") {
            let mut by: std::collections::HashMap<String, (u64, u64, u32)> = Default::default();
            let mut cur = String::new();
            // the unnamed anonymous regions (malloc arenas, thread stacks): size/resident kB, biggest resident first
            let mut anon: Vec<(u64, u64, String)> = Vec::new();
            let mut cur_size = 0u64;
            let mut cur_range = String::new();
            for l in smaps.lines() {
                let first = l.split_whitespace().next().unwrap_or("");
                if first.contains('-') && !first.ends_with(':') {
                    let mut it = l.split_whitespace();
                    let range = it.next().unwrap_or("");
                    let name = it.nth(4).unwrap_or("[anon]");
                    let (a, b) = range.split_once('-').unwrap_or(("0", "0"));
                    let size = u64::from_str_radix(b, 16).unwrap_or(0).saturating_sub(u64::from_str_radix(a, 16).unwrap_or(0)) / 1024;
                    cur = name.rsplit('/').next().unwrap_or(name).to_string();
                    cur_size = size;
                    cur_range = range.to_string();
                    let e = by.entry(cur.clone()).or_default();
                    e.1 += size;
                    e.2 += 1;
                } else if let Some(v) = l.strip_prefix("Pss:") {
                    let pss: u64 = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
                    by.entry(cur.clone()).or_default().0 += pss;
                } else if let Some(v) = l.strip_prefix("Rss:") {
                    let rss: u64 = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
                    if cur == "[anon]" && rss >= 1024 { anon.push((rss, cur_size, cur_range.clone())); }
                }
            }
            let mut v: Vec<_> = by.into_iter().collect();
            v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            for (n, (pss, size, count)) in v.into_iter().take(8) {
                out.push_str(&format!("{n}={pss}/{size}x{count} "));
            }
            anon.sort_by(|a, b| b.0.cmp(&a.0));
            out.push_str("| anon regions rss/size: ");
            for (rss, size, range) in anon.iter().take(24) { out.push_str(&format!("{rss}/{size}@{} ", range.split('-').next().unwrap_or(""))); }
        }
    }
    // elsewhere the OS's own figures (macOS: physical footprint, compressed, malloc zones)
    #[cfg(not(target_os = "linux"))]
    {
        out.push_str(&crate::memstat::process_rss().2);
    }
    out
}

/// Bytes resident on Unix (mincore), or committed on Windows (VirtualQuery).
pub fn block_committed(base: u64) -> u64 {
    if base == 0 { return 0; }
    #[cfg(unix)]
    {
        extern "C" {
            fn getpagesize() -> i32;
            fn mincore(addr: *mut u8, len: usize, vec: *mut u8) -> i32;
        }
        let page = unsafe { getpagesize() } as usize;
        let mut v = vec![0u8; BLOCK_SIZE / page];
        if unsafe { mincore(base as *mut u8, BLOCK_SIZE, v.as_mut_ptr()) } == 0 {
            return v.iter().filter(|b| **b & 1 != 0).count() as u64 * page as u64;
        }
    }
    #[cfg(windows)]
    {
        use crate::os::win32::{MemoryBasicInformation, VirtualQuery};
        let (mut at, end) = (base as usize, base as usize + BLOCK_SIZE);
        let mut committed = 0;
        while at < end {
            let mut info: MemoryBasicInformation = unsafe { std::mem::zeroed() };
            if unsafe { VirtualQuery(at as *const u8, &mut info, std::mem::size_of::<MemoryBasicInformation>()) } == 0 { break; }
            let next = (info.base as usize).saturating_add(info.region_size).min(end);
            if next <= at { break; }
            if info.state == crate::os::win32::MEM_COMMIT { committed += next - at; }
            at = next;
        }
        return committed as u64;
    }
    #[cfg(not(windows))]
    { let _ = base; 0 }
}

// ---- start

fn fail(id: DomainId, e: Vec<String>) -> Result<DomainId, Vec<String>> {
    if crate::run_state::observed() && !cancelled(id) {
        for error in &e { crate::run_state::diagnostic(Some(id), crate::diag::from_legacy(error, "run")); }
    }
    if cancelled(id) { set_state(id, DomainState::Stopped); return Ok(id); }
    else { set_state(id, DomainState::Crashed(e.first().cloned().unwrap_or_default())); }
    // The host can read these after the loader's compiler-thread guard has ended.
    Err(crate::zone_alloc::kept_clone(&e))
}

/// Starts an app as a domain of the running unit `up` (the `makepad run` unit, or any unit that
/// was set up as one): resolves its package with the code `makepad run` uses, loads the crates the
/// unit does not have yet (a crate is shared when its identity matches: `identities`), makes the
/// domain's static block, and runs the app's `main` on a thread of its own in that domain.
/// Returns once main is called; progress and the end are reported through `state` / the observer.
/// Host-native and privileged: only the host calls it.
pub fn start(up: *mut Unit, spec: AppSpec) -> Result<DomainId, Vec<String>> {
    let id = prepare(&spec)?;
    start_prepared(up, spec, id)
}

/// Registration is synchronous, so HAL stop cannot race ahead of the loader's first instruction.
pub fn prepare(spec: &AppSpec) -> Result<DomainId, Vec<String>> {
    let t0 = std::time::Instant::now();
    let id = {
        let mut d = domains();
        let id = spec.id.unwrap_or_else(|| d.iter().map(|x| x.info.id + 1).max().unwrap_or(1));
        // a relaunch starts the app again under its id: the ended run's record and block go
        if let Some(k) = d.iter().position(|x| x.info.id == id) {
            let old = &d[k];
            if id == 0 || old.compiling || old.reclaiming || !old.threads.is_empty() || old.info.base != 0 || matches!(old.info.state, DomainState::Leaked { .. }) {
                return Err(vec![format!("domain {id} still owns its previous run")]);
            }
            d.remove(k);
        }
        d.push(Domain {
            info: DomainInfo { id, package: spec.package.clone(), parent: spec.parent, state: DomainState::Compiling("package graph".into()), base: 0, crates: 0, reused: 0, loaded: 0, new_crates: Vec::new(), resolve_ms: 0.0, load_ms: 0.0, spawn_ms: 0.0, first_frame_ms: None, threads: 0, thread_names: Vec::new(), arena_in_use: 0, arena_committed: 0, arena_detail: (0, 0, 0), static_committed: 0 },
            started: t0,
            args: spec.argv.clone().unwrap_or_else(|| std::iter::once(spec.package.clone().into_bytes()).chain(spec.args.iter().map(|a| a.clone().into_bytes())).collect()),
            thread_done: false,
            compiling: true, reclaiming: false, cancel: false, threads: Vec::new(), heap: None,
        });
        id
    };
    Ok(id)
}

pub fn loading_failed(id: DomainId, message: &str) {
    let _loading = Loading(id);
    let _ = fail(id, vec![message.into()]);
}
struct Loading(DomainId);
impl Drop for Loading {
    fn drop(&mut self) {
        if let Some(d) = domains().iter_mut().find(|d| d.info.id == self.0) { d.compiling = false; }
        teardown(self.0);
        CHANGED.notify_all();
    }
}

fn start_prepared(up: *mut Unit, spec: AppSpec, id: DomainId) -> Result<DomainId, Vec<String>> {
    // New loaders wait outside the compiler phase while a swap has the gate shut. Every
    // entry path registers active loaders so the swap waits for their compile state.
    let _job = crate::jit::compile_job();
    let _z = crate::zone_alloc::compiler_thread();
    if crate::preload::trace() { eprintln!("boot-trace: domain {id} start_prepared at t={:.1} ms", crate::clock::ms_since_start()); }
    let _loading = Loading(id);
    let t0 = std::time::Instant::now();
    // The app's first start takes the boot's resolved package (same process, same instant). Taken
    // before anything can end this start: the slot never outlives it (its memory is the compiler
    // generation's, which the settle swap destroys), so a relaunch always resolves its own.
    let boot_resolved = if crate::run::app_domain() == Some(id) { crate::run::take_boot_resolved() } else { None };
    if cancelled(id) { return Ok(id); }
    set_state(id, DomainState::Compiling("package graph".into()));
    let sess = unsafe { (*std::ptr::addr_of!((*up).prog.session)).clone() };
    let workspace = crate::run::workspace_dir();
    let target_dir = crate::dirs::relax_dir().display().to_string();
    let std_dir = crate::std_dir();
    let from_boot = boot_resolved.is_some();
    let resolved = match boot_resolved {
        Some(r) => r,
        None => match crate::run::resolve_package(&sess, &workspace, &spec.package, &spec.cargo_flags, &spec.units_json, &target_dir, &std_dir, crate::run::ResolveFor::Domain(id), false) {
            Ok(r) => r,
            Err(e) => return fail(id, vec![e]),
        },
    };
    let resolve_ms = t0.elapsed().as_secs_f64() * 1e3;
    if crate::preload::trace() { eprintln!("boot-trace: domain {id} resolved at t={:.1} ms ({resolve_ms:.1} ms)", crate::clock::ms_since_start()); }
    if compile_phase(id, "loading crates").is_err() { return Ok(id); }
    let crate::run::Resolved { g, order, cfgs, live_inputs, .. } = resolved;
    let idents = identities(&g, &cfgs, &order);
    let t1 = std::time::Instant::now();
    let upn = up as usize;
    // the owner thread: the program's tables change there
    let loaded = crate::owner::call(crate::owner::Class::Program, || unsafe { load_graph(upn as *mut Unit, &g, &order, &cfgs, &idents, id, !from_boot) });
    let (root, n_reused, new_names, first_file) = match loaded {
        Ok(x) => x,
        Err(e) => return fail(id, e),
    };
    crate::run_state::emit(crate::run_state::Event::Warm { domain: Some(id), warm: new_names.is_empty() });
    let load_ms = t1.elapsed().as_secs_f64() * 1e3;
    if crate::preload::trace() { eprintln!("boot-trace: domain {id} loaded ({load_ms:.1} ms) at t={:.1} ms", crate::clock::ms_since_start()); }
    if compile_phase(id, "link").is_err() { return Ok(id); }
    let t2 = std::time::Instant::now();
    // the libraries the new crates link
    let (_, missing) = crate::run::link_libraries(unsafe { &(*up).prog }, &g.link_libs, g.native.as_ref(), first_file);
    if !missing.is_empty() && crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!("makepad-run domain: libraries not found: {:?}", missing);
    }
    // the macro world only the load needed goes again (hot reload and later loads rebuild it)
    if crate::pmhost::free_world() {
        std::thread::spawn(crate::memstat::trim);
    }
    if compile_phase(id, "main").is_err() { return Ok(id); }
    let prepared = crate::owner::call(crate::owner::Class::Program, || unsafe {
        if cancelled(id) { return Err("domain compilation cancelled".into()); }
        let u = &mut *(upn as *mut Unit);
        if crate::run::app_domain() == Some(id) { crate::layout::set_root_crate(root); }
        let main_def = crate::run::find_main(&u.prog, root).ok_or_else(|| format!("no `fn main` at the root of {}", spec.package))?;
        let fid = u.fn_id(crate::jit::FnKey::Inst(main_def, Vec::new()));
        let entry = u.entry(fid);
        let (target, arg) = crate::run::main_call(u, main_def, entry)?;
        Ok::<_, String>((target, arg, create(u)))
    });
    if crate::preload::trace() { eprintln!("boot-trace: domain {id} main prepared at t={:.1} ms", crate::clock::ms_since_start()); }
    let (target, arg, base) = match prepared {
        Ok(x) => x,
        Err(e) => return fail(id, vec![e]),
    };
    // the domain's arena is program memory: the kept heap (the compiler arena's generation is
    // destroyed by the settle swap; a domain heap is never touched by it)
    let heap = match crate::zone_alloc::kept_with(crate::zone_alloc::domain::DomainHeap::new) {
        Some(h) => h,
        None => { destroy(base); return fail(id, vec!["reserving domain heap".into()]); }
    };
    {
        let mut d = domains();
        if let Some(x) = d.iter_mut().find(|x| x.info.id == id) {
            x.heap = Some(heap);
            x.info.base = base;
            x.info.crates = g.crates.len();
            x.info.reused = n_reused;
            x.info.loaded = new_names.len();
            x.info.new_crates = crate::zone_alloc::kept_clone(&new_names);
            x.info.resolve_ms = resolve_ms;
            x.info.load_ms = load_ms;
            x.info.spawn_ms = t2.elapsed().as_secs_f64() * 1e3;
        }
    }
    if cancelled(id) { return Ok(id); }
    if crate::run::app_domain() == Some(id) {
        crate::reload::domain_loaded(live_inputs);
    }
    // the domain's main thread (its name, its handle) outlives any compiler generation
    if crate::preload::trace() { eprintln!("boot-trace: domain {id} launching at t={:.1} ms", crate::clock::ms_since_start()); }
    let launched = crate::zone_alloc::kept_with(|| launch_main(upn, id, base, target, arg, &spec.package, spec.around.clone()));
    if let Err(e) = launched {
        return fail(id, vec![e]);
    }
    Ok(id)
}

fn launch_main(upn: usize, id: DomainId, base: u64, target: u64, arg: u64, package: &str, around: Option<Around>) -> Result<std::thread::JoinHandle<()>, String> {
    let token = relax_domain_reserve(base);
    let name = format!("domain-{id}-{package}");
    if let Some(d) = domains().iter_mut().find(|d| d.info.id == id) {
        if let Some(t) = d.threads.iter_mut().find(|t| t.0 == token) { t.1 = name.clone(); }
    }
    let spawned = std::thread::Builder::new().name(name).stack_size(64 << 20).spawn(move || {
        crate::jit::ensure_altstack();
        if crate::run::app_domain() == Some(id) {
            crate::jit::MAIN_TID.store(crate::jit::current_thread_id(), std::sync::atomic::Ordering::Relaxed);
        }
        let up = upn as *mut Unit;
        relax_domain_enter(base, token);
        let mut run = || {
            // A stop between spawning and entry must never run app code.
            {
                let mut ds = domains();
                let d = ds.iter_mut().find(|d| d.info.id == id).expect("starting domain");
                if d.cancel { return; }
                d.info.state = DomainState::Running;
            }
            notify_state(id, &DomainState::Running);
            match unsafe { crate::jit::call_jit_in(up, base, target, arg) } {
                Ok(_) => end(id, DomainState::Exited(0)),
                // `process::exit` on the domain's thread (exit_current) already ended it
                Err(p) if p.kind == EXIT_KIND => {}
                Err(p) => end(id, DomainState::Crashed(format!("{}: {}", p.kind, p.message))),
            }
        };
        match &around {
            Some(a) => a(&mut run),
            None => run(),
        }
        if let Some(x) = domains().iter_mut().find(|x| x.info.id == id) {
            x.thread_done = true;
        }
        CHANGED.notify_all();
    });
    match spawned {
        Ok(h) => Ok(h),
        Err(e) => { thread_finished(base, token); Err(format!("starting the domain's thread: {e}")) }
    }
}

/// The kind of the `PanicInfo` a domain thread's `process::exit` leaves with (the code is its message).
pub(crate) const EXIT_KIND: &str = "domain exit";

/// The domain ends in `st` (exited or crashed); a domain that already ended keeps its first end.
fn end(id: DomainId, st: DomainState) {
    if matches!(state(id), Some(DomainState::Compiling(_) | DomainState::Running)) {
        set_state(id, st);
    }
}

/// The id of the app domain whose block is the calling thread's (None: the compile domain, or a
/// thread that is in none).
pub(crate) fn current_app_domain() -> Option<DomainId> {
    let base = current();
    domains().iter().find(|d| d.info.base == base && d.info.id != 0).map(|d| d.info.id)
}

/// `std::process::exit(code)` on a thread of an app domain ends that domain, not the process:
/// the domain is Exited(code) and the thread leaves its run (the frames' cleanups do not run, as
/// for an exit). False on any other thread: the caller exits the process.
pub(crate) fn exit_current(code: i32) -> bool {
    let Some(id) = current_app_domain() else { return false };
    end(id, DomainState::Exited(code));
    crate::jit::leave_thread(EXIT_KIND, code.to_string())
}

/// A compile error or fault the run cannot go on past (`COMPILE_ERRORS_FATAL`): in an app domain
/// it crashes that domain only. False on any other thread: the caller ends the process.
pub(crate) fn crash_current(msg: &str) -> bool {
    let Some(id) = current_app_domain() else { return false };
    end(id, DomainState::Crashed(msg.to_string()));
    true
}

/// Whether the calling thread runs an app domain (not the process's own app).
pub(crate) fn in_app_domain() -> bool {
    current_app_domain().is_some()
}

/// Waits for compilation, all registered threads and reclamation to finish.
pub fn wait_thread_done(id: DomainId, timeout: std::time::Duration) -> bool {
    let end = std::time::Instant::now() + timeout;
    let mut d = domains();
    loop {
        match d.iter().find(|x| x.info.id == id) {
            Some(x) if x.compiling || x.reclaiming || !x.threads.is_empty() || x.info.base != 0 && !matches!(x.info.state, DomainState::Leaked { .. }) => {}
            _ => return true,
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        d = d.wait_timeout(left);
    }
}

/// The host starts an app as a domain of the program `makepad run` booted (`start` on its unit).
pub fn start_prepared_in_run(spec: AppSpec, id: DomainId) -> Result<DomainId, Vec<String>> {
    let up = crate::run::running_unit();
    if up.is_null() { let _loading = Loading(id); return fail(id, vec!["no program is running".into()]); }
    start_prepared(up, spec, id)
}
pub fn start_in_run(spec: AppSpec) -> Result<DomainId, Vec<String>> {
    let up = crate::run::running_unit();
    if up.is_null() {
        return Err(vec!["no program is running".to_string()]);
    }
    start(up, spec)
}

/// On the owner thread: gets the crates of `g` into the program (shared ones are reused) and
/// returns (root crate, crates reused, names of the crates loaded, first file index of the load).
unsafe fn load_graph(up: *mut Unit, g: &crate::unitgraph::Graph, order: &[usize], cfgs: &[crate::cfg::CfgSet], idents: &[u64], id: DomainId, reread: bool) -> Result<(u32, usize, Vec<String>, usize), Vec<String>> {
    use crate::modules::{Dep, NewCrate};
    let u = &mut *up;
    let std_count = crate::std_pre_crates(&u.prog.session.clone(), u.prog.std_simd).len();
    let std_deps: Vec<(String, Dep)> = (0..std_count).map(|k| (u.prog.syms.str(u.prog.crates[k].name).to_string(), Dep::Existing(k as u32))).collect();
    let mut crate_of = vec![u32::MAX; g.crates.len()];
    let mut batch_of = vec![usize::MAX; g.crates.len()];
    let mut batch: Vec<NewCrate> = Vec::new();
    let mut batch_ci: Vec<usize> = Vec::new();
    let mut reused = 0;
    let mut why_new: Vec<String> = Vec::new();
    // A layout/signature edit cannot reuse the old compiled crate just because its package
    // identity stayed the same. Keep unchanged code shared; load changed crates and their
    // dependants as a new revision. Other domains can still finish using their old revision.
    // A domain that resolved its own package compares every loaded file of a shared crate with the
    // disk. The app's first start takes the boot's graph, whose files this process read a moment
    // ago: nothing to compare (a save after that is the live watcher's, which stamped them before
    // the load).
    let mut changed = HashSet::new();
    if reread {
        let candidates: HashSet<_> = idents.iter().filter_map(|i| lookup_loaded(*i)).collect();
        let mut current = std::collections::BTreeMap::new();
        for module in &u.prog.mods {
            let file = module.file as usize;
            let Some(&krate) = u.prog.file_crate.get(file) else { continue };
            if candidates.contains(&krate) {
                if let Some(path) = u.prog.file_paths.get(file) { current.insert((krate, path), file); }
            }
        }
        // every changed file, with the bytes on disk now: the program's source registry keeps the
        // first snapshot of a path for good (source ids name paths across revisions), so the new
        // revision reads these through an overlay -- without it a second relaunch after an edit
        // loaded the first relaunch's text again
        let mut fresh: Vec<(String, Vec<u8>)> = Vec::new();
        for ((krate, path), file) in current {
            if path.is_empty() { continue; }
            let pf = &u.prog.files[file];
            match crate::vfs::read(path) {
                Ok(src) if src.as_slice() == &pf.src[..pf.src_len as usize] => {}
                Ok(src) => {
                    changed.insert(krate);
                    if !fresh.iter().any(|f| &f.0 == path) { fresh.push((path.clone(), src)); }
                }
                Err(_) => { changed.insert(krate); }
            }
        }
        for (path, src) in fresh {
            u.prog.sources = std::sync::Arc::new(u.prog.sources.overlay(&path, src.into()));
        }
    }
    for &ci in order {
        let dependency_changed = g.crates[ci].deps.iter().any(|(d, _)| batch_of[*d] != usize::MAX);
        if let Some(k) = lookup_loaded(idents[ci]).filter(|k| !dependency_changed && !changed.contains(k)) {
            crate_of[ci] = k;
            reused += 1;
            continue;
        }
        let c = &g.crates[ci];
        why_new.push(namesake_diff(c, &cfgs[ci]).unwrap_or_else(|| c.name.clone()));
        let mut deps = std_deps.clone();
        for (d, ext) in &c.deps {
            if crate_of[*d] != u32::MAX {
                deps.push((ext.clone(), Dep::Existing(crate_of[*d])));
            } else if batch_of[*d] != usize::MAX {
                deps.push((ext.clone(), Dep::New(batch_of[*d])));
            }
        }
        batch_of[ci] = batch.len();
        batch_ci.push(ci);
        batch.push(NewCrate { name: c.name.clone(), path: c.src_path.clone(), edition: c.edition, cfg: cfgs[ci].clone(), deps, proc_macro: c.proc_macro });
        // (env!() looks a crate up by name: a second crate of a name replaces the first one's
        // environment, which differs here by definition of a new identity)
        crate::crates::set_crate_env(&c.name, g.env[ci].clone());
    }
    let first_file = u.prog.files.len();
    if !batch.is_empty() {
        compile_phase(id, "load").map_err(|e| vec![e])?;
        let added = crate::modules::add_crates(up, &batch, true, Some(id))?;
        // Loaded crates remain reusable even when stop arrived during collection.
        // As for a boot: unresolved imports and signature errors are reported, the load goes on
        // (only a crate that does not parse is refused).
        if !added.errs.is_empty() {
            if crate::run_state::observed() {
                for e in &added.errs { crate::run_state::diagnostic(Some(id), crate::diag::from_legacy(e, "typeck")); }
            }
            let max: usize = std::env::var("MAKEPAD_RUN_ERRORS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
            eprintln!("makepad-run domain: {} diagnostics in the loaded crates (first {}):", added.errs.len(), max.min(added.errs.len()));
            for e in added.errs.iter().take(max) {
                eprintln!("  {}", e);
            }
        }
        for (b, &ci) in batch_ci.iter().enumerate() {
            crate_of[ci] = added.crates[b];
            record_loaded(idents[ci], added.crates[b], &g.crates[ci], &cfgs[ci]);
        }
        // provided crates (rustc-built into the loader) that only this app brings, like the
        // compute crate for Stage, bind their ops now, as the booted program's do
        { let _fw = crate::jit::front_write(); u.bind_added_provided(&added.crates); }
    }
    if cancelled(id) { return Err(vec!["domain compilation cancelled".into()]); }
    let root = crate_of[g.root];
    if root == u32::MAX {
        return Err(vec![format!("the package root {} did not load", g.crates[g.root].name)]);
    }
    Ok((root, reused, why_new, first_file))
}

// ---- the test vehicle: `makepad run <package> --domain <package2> ..`

/// Starts each of `packages` as a domain, one after the other, once the primary app presented its first
/// frame (or after 3 minutes), and reports on stderr for each: the crates shared and loaded, the
/// time to its first frame, the process's memory before the start, at its first frame and 10 s
/// later, and what its static block holds.
pub(crate) fn run_vehicle(up: *mut Unit, packages: Vec<String>) {
    let upn = up as usize;
    // The vehicle outlives the settle swap: its package names, its thread (name) and what it
    // prints are kept state, not the compiler generation the caller is in.
    let _k = crate::zone_alloc::kept();
    let packages = packages.clone();
    let _ = std::thread::Builder::new().name("run-domains".to_string()).spawn(move || {
        let up = upn as *mut Unit;
        let wait = std::time::Duration::from_secs(180);
        let primary = crate::run::app_domain().unwrap_or(0);
        let ff0 = wait_first_frame(primary, wait);
        let (rss, pss) = process_memory_kb();
        eprintln!("makepad-run domain: id={primary} first_frame_ms={} rss_kb={} pss_kb={}", ff0.map_or("none".to_string(), |x| format!("{x:.0}")), rss, pss);
        for pkg in packages {
            let (rss0, pss0) = process_memory_kb();
            let t = std::time::Instant::now();
            let id = match start(up, AppSpec { package: pkg.clone(), ..Default::default() }) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("makepad-run domain: {pkg}: start failed: {}", e.join(" | "));
                    continue;
                }
            };
            let started_ms = t.elapsed().as_secs_f64() * 1e3;
            let ff = wait_first_frame(id, wait);
            let (rss1, pss1) = process_memory_kb();
            std::thread::sleep(std::time::Duration::from_secs(10));
            let (rss2, pss2) = process_memory_kb();
            let i = info(id).unwrap();
            eprintln!(
                "makepad-run domain: id={id} pkg={pkg} state={:?} crates={} reused={} new={} start_call_ms={started_ms:.0} resolve_ms={:.0} load_ms={:.0} spawn_ms={:.0} first_frame_ms={} block_committed_kb={} new_crates=[{}] rss_kb={rss0}/{rss1}/{rss2} pss_kb={pss0}/{pss1}/{pss2}",
                i.state, i.crates, i.reused, i.loaded, i.resolve_ms, i.load_ms, i.spawn_ms,
                ff.map_or("none".to_string(), |x| format!("{x:.0}")), block_committed(i.base) / 1024, i.new_crates.join(",")
            );
        }
        for d in list() {
            eprintln!("makepad-run domain: list id={} pkg={} state={:?} first_frame_ms={:?}", d.id, d.package, d.state, d.first_frame_ms.map(|x| x.round()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jit::FnKey;
    use crate::program::DefId;

    /// Two domains on the same compiled code: each has its own statics, a pointer inside a static
    /// image points into its own block, a static laid out after a domain exists is copied into it.
    #[test]
    fn domains_have_their_own_statics() {
        isolated_test!(domains_have_their_own_statics);
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-domains-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, r#"
            static mut COUNTER: u64 = 5;
            static mut CELL: u64 = 1;
            static PCELL: *mut u64 = unsafe { &CELL as *const u64 as *mut u64 };
            static mut LATE: u64 = 40;
            fn bump(out: *mut u64) { unsafe { COUNTER += 1; *out = COUNTER; } }
            fn through(out: *mut u64) { unsafe { *PCELL += 10; *out = CELL; } }
            fn late(out: *mut u64) { unsafe { LATE += 1; *out = LATE; } }
        "#).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |u: &Unit, name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (bump, through, late) = (def(&u, "bump"), def(&u, "through"), def(&u, "late"));
        let (counter, cell, pcell, late_static) = (def(&u, "COUNTER"), def(&u, "CELL"), def(&u, "PCELL"), def(&u, "LATE"));
        let up = &mut *u as *mut Unit;
        let entry = |u: &mut Unit, d: DefId| { let id = u.fn_id(FnKey::Inst(d, Vec::new())); u.entry(id) };
        let call = |base: u64, entry: u64| -> u64 {
            let mut out = 0u64;
            unsafe { crate::jit::call_jit_in(up, base, entry, &mut out as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
            out
        };
        let dom0 = u.syms.dom0();
        // the statics exist (and have offsets) before the second domain is made
        for d in [counter, cell, pcell] {
            let s = u.static_sym(d);
            assert!(u.syms.value(s) > 0 && u.syms.value(s) < BLOCK_SIZE as u64, "an offset, not an address");
            assert_eq!(u.syms.addr(s), dom0 + u.syms.value(s));
        }
        let (e_bump, e_through) = (entry(&mut u, bump), entry(&mut u, through));
        assert_eq!(call(dom0, e_bump), 6);
        let b = create(&u);
        assert_ne!(b, dom0);
        // the new domain starts from the INITIAL image (5), not the running state (6)
        assert_eq!(call(b, e_bump), 6);
        assert_eq!(call(b, e_bump), 7);
        assert_eq!(call(dom0, e_bump), 7);
        assert_eq!(call(dom0, e_bump), 8);
        assert_eq!(call(b, e_bump), 8);
        // PCELL holds the address of CELL: in the domain's own block
        assert_eq!(call(dom0, e_through), 11);
        assert_eq!(call(b, e_through), 11);
        assert_eq!(call(b, e_through), 21);
        assert_eq!(call(dom0, e_through), 21);
        let ps = u.static_sym(pcell);
        let cs = u.static_sym(cell);
        let (pcell_off, cell_off) = (u.syms.value(ps), u.syms.value(cs));
        unsafe {
            assert_eq!(*((dom0 + pcell_off) as *const u64), dom0 + cell_off);
            assert_eq!(*((b + pcell_off) as *const u64), b + cell_off);
        }
        // a static first named after the domain exists is copied into it
        let e_late = entry(&mut u, late);
        assert!(u.statics.get(&late_static).is_none());
        assert_eq!(call(dom0, e_late), 41);
        assert_eq!(call(b, e_late), 41);
        assert_eq!(call(b, e_late), 42);
        assert_eq!(call(dom0, e_late), 42);
        // the thread is back in the domain it was in
        assert_eq!(current(), dom0);
        assert_eq!(enter(b), dom0);
        assert_eq!(enter(dom0), b);
        destroy(b);
        assert!(!blocks().contains(&b));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A static whose array length is a named constant (`[u8; N]`) is laid out and captured at its
    /// full size: a domain created afterwards starts from all N bytes of the initial image (the
    /// image was once sized by an unnormalized type, so a domain saw the first byte only).
    #[test]
    fn static_array_with_const_length_keeps_its_image_in_a_domain() {
        isolated_test!(static_array_with_const_length_keeps_its_image_in_a_domain);
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-domains-arr-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, r#"
            const N: usize = 5;
            static TABLE: [u8; N] = [3, 1, 4, 1, 5];
            fn sum(out: *mut u64) { let mut s = 0u64; let mut i = 0; while i < N { s = s * 10 + TABLE[i] as u64; i += 1; } unsafe { *out = s; } }
        "#).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |u: &Unit, name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (sum, table) = (def(&u, "sum"), def(&u, "TABLE"));
        let up = &mut *u as *mut Unit;
        let _ = u.static_sym(table);
        let id = u.fn_id(FnKey::Inst(sum, Vec::new()));
        let entry = u.entry(id);
        let b = create(&u);
        let mut out = 0u64;
        unsafe { crate::jit::call_jit_in(up, b, entry, &mut out as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
        assert_eq!(out, 31415);
        destroy(b);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A constant pointing at a static (directly or through data it points to) is per domain:
    /// each domain's copy points at its own static; a constant that reaches no static stays shared.
    #[test]
    fn const_pointing_at_static_is_per_domain() {
        isolated_test!(const_pointing_at_static_is_per_domain);
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-domains-const-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("fixture.rs");
        std::fs::write(&source, r#"
            static mut S: u64 = 3;
            const DIRECT: &[*mut u64; 1] = &[unsafe { &raw mut S }];
            const NESTED: &[&[*mut u64; 1]; 1] = &[&[unsafe { &raw mut S }]];
            const PLAIN: &[u64; 2] = &[4, 5];
            fn direct(out: *mut u64) { unsafe { *DIRECT[0] += 1; *out = *DIRECT[0]; } }
            fn nested(out: *mut u64) { unsafe { *NESTED[0][0] += 10; *out = *NESTED[0][0]; } }
            fn plain(out: *mut u64) { unsafe { *out = PLAIN[1]; } }
        "#).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("fixture", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |u: &Unit, name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (direct, nested, plain) = (def(&u, "direct"), def(&u, "nested"), def(&u, "plain"));
        let up = &mut *u as *mut Unit;
        let entry = |u: &mut Unit, d: DefId| { let id = u.fn_id(FnKey::Inst(d, Vec::new())); u.entry(id) };
        let call = |base: u64, entry: u64| -> u64 {
            let mut out = 0u64;
            unsafe { crate::jit::call_jit_in(up, base, entry, &mut out as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
            out
        };
        let dom0 = u.syms.dom0();
        let (e_direct, e_nested, e_plain) = (entry(&mut u, direct), entry(&mut u, nested), entry(&mut u, plain));
        let b = create(&u);
        assert_eq!(call(dom0, e_direct), 4);
        assert_eq!(call(b, e_direct), 4);
        assert_eq!(call(b, e_nested), 14);
        assert_eq!(call(dom0, e_nested), 14);
        assert_eq!(call(dom0, e_direct), 15);
        assert_eq!(call(b, e_plain), 5);
        assert!(u.errors.is_empty(), "{:?}", u.errors);
        destroy(b);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod teardown_tests;

#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());

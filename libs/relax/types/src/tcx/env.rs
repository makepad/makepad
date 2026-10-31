use super::{DefId, Tcx};
use crate::next_solver::ParamEnv;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering::{Acquire, Relaxed, Release}};
use relax_session::sync::Mutex;

/// Tcx-local predicate identity. rustc_middle/src/ty/mod.rs:987 stores interned clauses.
/// Identity hashing follows rustc_data_structures/src/intern.rs:95, using an arena id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParamEnvId(u32);

impl ParamEnvId {
    pub const EMPTY: Self = Self(0);
}

/// rustc_type_ir/src/infer_ctxt.rs:107: Analysis carries an interned owner list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DefiningOpaquesId(u32);

#[derive(PartialEq, Eq, Hash)]
enum Data {
    Predicates(ParamEnv),
    Owners(Vec<DefId>),
}

struct Entry {
    data: Data,
    hash: u64,
    next: u32,
    flags: u32,
    canonical: std::sync::OnceLock<crate::next_solver::canonical::CanonicalParamEnvCacheEntry>,
}

const CHUNK: usize = 256;
const MAX_CHUNKS: usize = 4096;
const BUCKETS: usize = 4096;
const LOCKS: usize = 32;

/// Append-only storage, with the same publication and shard-lock scheme as Types.
/// rustc_middle/src/ty/context.rs:258 interns clauses before they enter query keys.
pub(super) struct Envs {
    chunks: Box<[AtomicPtr<Entry>]>,
    len: AtomicU32,
    grow: Mutex<()>,
    buckets: Box<[AtomicU32]>,
    locks: Box<[Mutex<()>]>,
}

// Entries never move; published data and flags are immutable. The canonical cell
// synchronizes its own initialization. All entries are freed only with their Tcx.
unsafe impl Send for Envs {}
unsafe impl Sync for Envs {}

impl Envs {
    pub(super) fn new() -> Self {
        let mut chunks = Vec::new();
        let mut buckets = Vec::new();
        let mut locks = Vec::new();
        for _ in 0..MAX_CHUNKS { chunks.push(AtomicPtr::new(std::ptr::null_mut())); }
        for _ in 0..BUCKETS { buckets.push(AtomicU32::new(0)); }
        for _ in 0..LOCKS { locks.push(Mutex::new(())); }
        let envs = Self { chunks: chunks.into_boxed_slice(), len: AtomicU32::new(0), grow: Mutex::new(()), buckets: buckets.into_boxed_slice(), locks: locks.into_boxed_slice() };
        assert_eq!(envs.intern(Data::Predicates(ParamEnv::default()), None), 0);
        envs
    }

    #[inline]
    fn entry(&self, id: u32) -> &Entry {
        let i = id as usize;
        assert!(i < self.len.load(Acquire) as usize);
        let chunk = self.chunks[i / CHUNK].load(Relaxed);
        // The length/bucket publication follows initialization; chunks never move.
        unsafe { &*chunk.add(i % CHUNK) }
    }

    fn find(&self, mut head: u32, hash: u64, data: &Data) -> Option<u32> {
        while head != 0 {
            let entry = self.entry(head - 1);
            if entry.hash == hash && entry.data == *data { return Some(head - 1); }
            head = entry.next;
        }
        None
    }

    fn intern(&self, data: Data, tys: Option<&crate::types::Types>) -> u32 {
        let mut hasher = crate::fxhash::FxHasher::default();
        data.hash(&mut hasher);
        let hash = hasher.finish();
        let bucket = (hash >> 52) as usize;
        if let Some(id) = self.find(self.buckets[bucket].load(Acquire), hash, &data) { return id; }
        // Compute before taking the interner locks: const flags may read the const table.
        let flags = match (&data, tys) {
            (Data::Predicates(env), Some(tys)) => env.clauses.iter().fold(0, |flags, p| flags | p.flags(tys)),
            _ => 0,
        };
        let _lock = self.locks[bucket % LOCKS].lock().unwrap_or_else(|e| e.into_inner());
        let head = self.buckets[bucket].load(Acquire);
        if let Some(id) = self.find(head, hash, &data) { return id; }
        let _grow = self.grow.lock().unwrap_or_else(|e| e.into_inner());
        let i = self.len.load(Relaxed) as usize;
        assert!(i / CHUNK < MAX_CHUNKS, "makepad: environment interner full");
        let mut chunk = self.chunks[i / CHUNK].load(Relaxed);
        if chunk.is_null() {
            let layout = std::alloc::Layout::array::<Entry>(CHUNK).unwrap();
            chunk = unsafe { std::alloc::alloc(layout) as *mut Entry };
            if chunk.is_null() { std::alloc::handle_alloc_error(layout); }
            self.chunks[i / CHUNK].store(chunk, Release);
        }
        unsafe { chunk.add(i % CHUNK).write(Entry { data, hash, next: head, flags, canonical: Default::default() }); }
        self.len.store(i as u32 + 1, Release);
        self.buckets[bucket].store(i as u32 + 1, Release);
        i as u32
    }
}

impl Envs {
    /// Memory report: (entries, bytes) of the arena chunks and the entries' arrays.
    pub(super) fn mem_stats(&self) -> (usize, usize) {
        let n = self.len.load(Acquire) as usize;
        let mut b = ((n + CHUNK - 1) / CHUNK) * CHUNK * std::mem::size_of::<Entry>();
        for i in 0..n {
            b += match &self.entry(i as u32).data {
                Data::Predicates(e) => e.clauses.len() * std::mem::size_of::<crate::next_solver::Predicate>(),
                Data::Owners(o) => o.capacity() * 4,
            };
        }
        (n, b)
    }
}

impl Drop for Envs {
    fn drop(&mut self) {
        let len = *self.len.get_mut() as usize;
        for (ci, chunk) in self.chunks.iter_mut().enumerate() {
            let ptr = *chunk.get_mut();
            if ptr.is_null() { continue; }
            let live = len.saturating_sub(ci * CHUNK).min(CHUNK);
            unsafe {
                for i in 0..live { std::ptr::drop_in_place(ptr.add(i)); }
                std::alloc::dealloc(ptr as *mut u8, std::alloc::Layout::array::<Entry>(CHUNK).unwrap());
            }
        }
    }
}

impl Tcx {
    /// rustc_middle/src/ty/context.rs mk_clauses: a typing environment's clause list is
    /// interned before it enters query keys (TypingEnv); the empty list is EMPTY.
    pub fn intern_param_env(&self, env: ParamEnv) -> ParamEnvId {
        crate::solver_work::count(crate::solver_work::ENV_INTERNS);
        if env.clauses.is_empty() { return ParamEnvId::EMPTY; }
        ParamEnvId(self.envs.intern(Data::Predicates(env), Some(&self.tys)))
    }

    #[inline]
    pub fn param_env_by_id(&self, id: ParamEnvId) -> &ParamEnv {
        match &self.envs.entry(id.0).data {
            Data::Predicates(env) => env,
            Data::Owners(_) => unreachable!(),
        }
    }

    pub(crate) fn param_env_flags(&self, id: ParamEnvId) -> u32 { self.envs.entry(id.0).flags }

    /// One canonicalization cell per interned key; no second map or global lock.
    pub(crate) fn canonical_param_env(&self, id: ParamEnvId) -> &std::sync::OnceLock<crate::next_solver::canonical::CanonicalParamEnvCacheEntry> {
        &self.envs.entry(id.0).canonical
    }

    pub fn intern_defining_opaques(&self, owners: Vec<DefId>) -> DefiningOpaquesId {
        if owners.is_empty() { return DefiningOpaquesId::default(); }
        DefiningOpaquesId(self.envs.intern(Data::Owners(owners), None))
    }

    pub fn defining_opaques(&self, id: DefiningOpaquesId) -> &[DefId] {
        if id.0 == 0 { return &[]; }
        match &self.envs.entry(id.0).data {
            Data::Owners(owners) => owners,
            Data::Predicates(_) => unreachable!(),
        }
    }
}

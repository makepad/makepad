//! Symbols: every run-dependent value compiled code uses (plans/relax-cache.md, C1).
//!
//! Code never holds an address, a panic site index or another per-run number as an immediate:
//! RIR names it as a `Sym` (an id into this table + an offset), the back end loads it from the
//! function's address pool and reports a relocation. The table maps each id to what it stands
//! for (`SymKind`) and to its value in this run (a u64 array the back ends read through
//! `Env::syms`). Code made in another run (the disk cache) is linked by looking its symbols up
//! here and rewriting the pool words.
//!
//! Data objects (literals, constants, vtables, `Location`s) are registered with what their
//! bytes hold, so the optimiser's constant-load folding (`ro_read`) can tell an integer from a
//! pointer: a pointer word folds to a symbolic address, never to a bare number; a word whose
//! meaning is unknown does not fold.

use crate::jit::Unit;
use crate::program::DefId;
use crate::rir::{Mem, Sym, SymClass};
use crate::types::{TyId, TyKind};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;
use relax_session::sync::Mutex;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum SymKind {
    /// a Relax runtime entry or glue routine, by name
    Rt(&'static str),
    /// a data object (`DataObj`), by index
    Data(u32),
    /// a static's memory
    Static(DefId),
    /// a C symbol (function or foreign static), by name
    Foreign(String),
    /// the panicking stand-in for a C symbol this host lacks, by name
    Missing(String),
    /// an `asm!` stub, by its assembly text
    Asm(String),
    /// a panic site (the value is the site's index)
    Site(u32),
    /// a function slot's permanent callable address (fn pointers stored in data)
    Thunk(u32),
    /// an anonymous mutable block (a promoted temporary lowering placed in static memory):
    /// index into `Syms::rw_blocks` (size, align)
    Rw(u32),
    /// an entry of a provided (host-built) crate: `shim:<key>`, `last_panic:<crate>`,
    /// `drop:<type def path>`
    Provided(String),
}

/// What the bytes of a data object hold.
#[derive(Clone, Debug)]
pub enum DataKind {
    /// plain bytes (string literals, file paths): no pointers
    Bytes,
    /// a value of this type: pointer words are found by walking the type
    Typed(TyId),
    /// 8-byte words, each a pointer (Some) or an integer (None): vtables, Locations
    Words(Vec<Option<Sym>>),
    /// Explicit byte offsets and targets, retained when linking cached data (also packed fields).
    Relocs(Vec<(u32, Sym)>),
}

pub struct DataObj {
    pub start: u64,
    pub len: u64,
    pub align: u64,
    pub kind: DataKind,
    pub sym: u32,
    /// stable identity (const path + args, vtable types, location); None: by content
    pub name: Option<String>,
    /// writable at run time (promoted values in statics): a pointer target, never folded
    pub mutable: bool,
    /// pointer words (offset -> target, None = a pointer whose target is unknown, or a word
    /// that cannot be classified), computed on the first fold that reads the object
    ptrs: Option<std::sync::Arc<Vec<(u32, Option<Sym>)>>>,
    /// the type walk could not classify the object (union, unknown ADT): nothing folds
    opaque: bool,
}

pub const SYM_CAP: usize = 1 << 22;

pub struct Syms {
    /// value table (u64 per id), the back ends' `Env::syms`
    pub vals: u64,
    /// `rir::SymClass` per id (u8): how a backend may encode the symbol (`Env::sym_class`)
    pub classes: u64,
    /// kind per id (append-only: read without a lock)
    kinds: crate::conc::AppendVec<SymKind>,
    inner: Mutex<SymsInner>,
    data: Mutex<DataIndex>,
    /// (size, align) of each `SymKind::Rw` block
    pub rw_blocks: Mutex<Vec<(u64, u64)>>,
    /// the writable blocks lowering made, by address: start -> (size, the value's type)
    /// (promoted_copy)
    pub rw_starts: Mutex<BTreeMap<u64, (u64, TyId)>>,
    /// start address -> symbol of each anonymous mutable block (`Unit::rw_sym`): a constant
    /// re-homed per domain (domain.rs rehome_const) is one, and lowering finds its symbol by address
    pub rw_syms: Mutex<BTreeMap<u64, Sym>>,
    /// the anonymous mutable blocks that hold a constant's value copied per domain
    /// (domain.rs rehome_const), by symbol: the start of the constant data object copied. Their
    /// bytes are the value, not storage code writes before it reads, so a cache entry names
    /// them as "the copy of that object" (cache.rs enc_sym tag 10), never as a fresh block
    pub const_copies: Mutex<HashMap<u32, u64>>,
    /// Declared initial bytes of statics and their promoted data, before host execution.
    pub initial_data: Mutex<HashMap<u32, Vec<u8>>>,
    /// the definition behind each Foreign / Missing symbol
    pub foreign_defs: Mutex<HashMap<u32, DefId>>,
    /// Reverse attribution is lazy and shared by every pointer word in this Unit.
    rt_by_addr: OnceLock<HashMap<u64, Sym>>,
    foreign_by_addr: Mutex<ForeignIndex>,
    /// The compile domain's static block (domain 0): the address its offsets are relative to.
    /// A domain symbol (a static, a writable block: `SymClass::Domain`) is interned with its
    /// address in that block and holds the offset; `addr` gives the address back.
    dom0: u64,
}

#[derive(Default)]
struct ForeignIndex {
    by_addr: HashMap<u64, Sym>,
    /// Program definitions are append-only, including hot reload / runtime modules.
    defs_scanned: usize,
}

/// Dedup map for symbols that can be asked for more than once (runtime entries, statics, C
/// symbols, thunks, asm stubs, panic sites -- one per site index); data objects and blocks are new every time.
#[derive(Default)]
struct SymsInner {
    map: HashMap<SymKind, u32>,
}

#[derive(Default)]
struct DataIndex {
    objs: Vec<DataObj>,
    by_start: BTreeMap<u64, u32>,
}

impl Syms {
    pub fn new(vals: u64, dom0: u64) -> Syms {
        Syms { dom0, vals, classes: crate::jit::alloc_rw(SYM_CAP), kinds: crate::conc::AppendVec::new_kept(SYM_CAP), inner: Mutex::new(SymsInner::default()), data: Mutex::new(DataIndex::default()), rw_blocks: Mutex::new(Vec::new()), rw_starts: Mutex::new(BTreeMap::new()), rw_syms: Mutex::new(BTreeMap::new()), const_copies: Mutex::new(HashMap::new()), initial_data: Mutex::new(HashMap::new()), foreign_defs: Mutex::new(HashMap::new()), rt_by_addr: OnceLock::new(), foreign_by_addr: Mutex::new(ForeignIndex::default()) }
    }

    /// The kept-state scan (jit/zone_scan.rs): the owned allocations of the symbol table.
    pub(crate) fn scan(&self, a: &mut crate::jit::zone_scan::Acc) {
        fn kind(a: &mut crate::jit::zone_scan::Acc, k: &SymKind) {
            match k {
                SymKind::Foreign(s) | SymKind::Missing(s) | SymKind::Asm(s) | SymKind::Provided(s) => a.string(s),
                _ => {}
            }
        }
        a.field("syms.kinds");
        self.kinds.alloc_ptrs(|p| a.ptr(p));
        for k in self.kinds.iter() {
            kind(a, k);
        }
        a.field("syms.map");
        if let Ok(g) = self.inner.try_lock() {
            a.map(&g.map, |a, k, _| kind(a, k));
        }
        a.field("syms.data");
        if let Ok(g) = self.data.try_lock() {
            a.vec(&g.objs);
            for o in g.objs.iter() {
                if let Some(n) = &o.name {
                    a.string(n);
                }
                match &o.kind {
                    DataKind::Words(w) => a.vec(w),
                    DataKind::Relocs(r) => a.vec(r),
                    _ => {}
                }
                if let Some(p) = &o.ptrs {
                    a.arc(p);
                    a.vec(p);
                }
            }
            a.btree(&g.by_start, |_, _, _| {});
        }
        a.field("syms.rw_blocks");
        if let Ok(g) = self.rw_blocks.try_lock() {
            a.vec(&g);
        }
        a.field("syms.rw_starts");
        if let Ok(g) = self.rw_starts.try_lock() {
            a.btree(&g, |_, _, _| {});
        }
        a.field("syms.initial_data");
        if let Ok(g) = self.initial_data.try_lock() {
            a.map(&g, |a, _, v| a.vec(v));
        }
        a.field("syms.foreign_defs");
        if let Ok(g) = self.foreign_defs.try_lock() {
            a.map(&g, |_, _, _| {});
        }
        a.field("syms.rt_by_addr");
        if let Some(m) = self.rt_by_addr.get() {
            a.map(m, |_, _, _| {});
        }
        a.field("syms.foreign_by_addr");
        if let Ok(g) = self.foreign_by_addr.try_lock() {
            a.map(&g.by_addr, |_, _, _| {});
        }
    }

    /// The symbol for `kind` (created with `value` on first use; later calls return the same id).
    pub fn intern(&self, kind: SymKind, value: u64) -> Sym {
        if matches!(kind, SymKind::Data(_) | SymKind::Rw(_)) {
            return self.push(kind, value);
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&id) = g.map.get(&kind) {
            return Sym::new(id);
        }
        // identity state: the kind (and its string) outlives the compiler arena
        let _k = crate::zone_alloc::kept();
        let own = kind.clone();
        let s = self.push(own.clone(), value);
        g.map.insert(own, s.id);
        s
    }

    fn push(&self, kind: SymKind, value: u64) -> Sym {
        // a domain symbol's table value is its offset in the domain static block (0: no storage)
        let value = if class_of(&kind) == crate::rir::SymClass::Domain && value != 0 { value - self.dom0 } else { value };
        let id = self.kinds.push(kind) as u32;
        if id as usize >= SYM_CAP {
            panic!("symbol table full");
        }
        unsafe { std::ptr::write((self.vals as *mut u64).add(id as usize), value) };
        let class = class_of(&self.kinds[id as usize]);
        unsafe { std::ptr::write((self.classes as *mut u8).add(id as usize), class as u8) };
        Sym::new(id)
    }

    pub fn kind(&self, id: u32) -> Option<SymKind> {
        if (id as usize) < self.kinds.len() {
            Some(self.kinds[id as usize].clone())
        } else {
            None
        }
    }

    /// The symbol's table value: what code relocated against it holds. For a domain symbol that
    /// is the offset in a domain's static block; use `addr` for a pointer to it.
    pub fn value(&self, s: Sym) -> u64 {
        unsafe { s.value(self.vals) }
    }

    /// The address a pointer to the symbol holds in memory the compile domain (domain 0) owns:
    /// its value, in the domain static block for a domain symbol.
    pub fn addr(&self, s: Sym) -> u64 {
        let v = self.value(s);
        // a static whose storage failed has the value 0 (no offset is 0: the block starts later)
        let base = v.wrapping_sub(s.off as u64);
        if SymClass::of(self.classes, s.id) == SymClass::Domain && base != 0 {
            v.wrapping_add(self.dom0)
        } else {
            v
        }
    }

    /// The compile domain's static block.
    pub fn dom0(&self) -> u64 {
        self.dom0
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Registers the data object at `start..start+len` (Unit::data); returns its symbol.
    pub fn add_data(&self, start: u64, len: u64, align: u64, kind: DataKind, mutable: bool) -> Sym {
        // one critical section: the object's index and its symbol must agree (lock order:
        // data, then the symbol table)
        let mut d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let _k = crate::zone_alloc::kept();
        let kind = kind.clone();
        let idx = d.objs.len() as u32;
        let s = self.intern(SymKind::Data(idx), start);
        d.objs.push(DataObj { start, len, align, kind, sym: s.id, name: None, mutable, ptrs: None, opaque: false });
        d.by_start.insert(start, idx);
        s
    }

    /// Registers data objects placed together (cache/image.rs place_data) under one lock:
    /// (start, len, align, name) each, read-only; `ptrs(i, syms)`: object `i`'s pointer words,
    /// given the symbol of every object of the batch.
    pub fn add_data_batch(&self, objs: &[(u64, u64, u64, Option<String>)], mut ptrs: impl FnMut(usize, &[Sym]) -> Vec<(u32, Sym)>) -> Vec<Sym> {
        let mut d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let _k = crate::zone_alloc::kept();
        d.objs.reserve(objs.len());
        let first = d.objs.len() as u32;
        let syms: Vec<Sym> = objs.iter().enumerate().map(|(i, o)| self.intern(SymKind::Data(first + i as u32), o.0)).collect();
        for (i, (start, len, align, name)) in objs.iter().enumerate() {
            let kind = DataKind::Relocs(ptrs(i, &syms));
            d.objs.push(DataObj { start: *start, len: *len, align: *align, kind, sym: syms[i].id, name: name.clone(), mutable: false, ptrs: None, opaque: false });
            d.by_start.insert(*start, first + i as u32);
        }
        syms
    }

    /// The anonymous mutable block (`Unit::rw_sym`) containing `addr`, as its symbol + offset.
    pub(crate) fn rw_sym_at(&self, addr: u64) -> Option<Sym> {
        let (start, sym) = {
            let m = self.rw_syms.lock().unwrap_or_else(|e| e.into_inner());
            let (&start, &sym) = m.range(..=addr).next_back()?;
            (start, sym)
        };
        let Some(SymKind::Rw(k)) = self.kind(sym.id) else { return None };
        let size = self.rw_blocks.lock().unwrap_or_else(|e| e.into_inner()).get(k as usize)?.0;
        (addr < start + size.max(1)).then(|| Sym { id: sym.id, off: (addr - start) as i64 })
    }

    /// The data object containing `addr`: (start, len, align, mutable).
    pub(crate) fn object_of(&self, addr: u64) -> Option<(u64, u64, u64, bool)> {
        if addr <= 0x10000 {
            return None;
        }
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let (&start, &i) = d.by_start.range(..=addr).next_back()?;
        let o = &d.objs[i as usize];
        (addr < start + o.len.max(1)).then(|| (o.start, o.len, o.align, o.mutable))
    }

    /// What the data object starting at `start` holds (`Bytes` when unknown).
    pub(crate) fn kind_at(&self, start: u64) -> DataKind {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        d.by_start.get(&start).map(|&i| d.objs[i as usize].kind.clone()).unwrap_or(DataKind::Bytes)
    }

    /// The symbol of the data object starting exactly at `start`.
    pub fn data_at(&self, start: u64) -> Option<Sym> {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        d.by_start.get(&start).map(|&i| Sym::new(d.objs[i as usize].sym))
    }

    /// Sets what a registered object holds (its bytes are written after `Unit::data`).
    pub fn set_data_kind(&self, start: u64, kind: DataKind) {
        let _k = crate::zone_alloc::kept();
        let kind = kind.clone();
        let mut d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&i) = d.by_start.get(&start) {
            let o = &mut d.objs[i as usize];
            o.kind = kind;
            o.ptrs = None;
            o.opaque = false;
        }
    }

    /// Names a registered object (its stable identity across runs).
    pub fn set_data_name(&self, start: u64, name: String) {
        let _k = crate::zone_alloc::kept();
        let name = name.clone();
        let mut d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&i) = d.by_start.get(&start) {
            d.objs[i as usize].name = Some(name);
        }
    }

    /// The runtime entry named `name` (RtSyms field name).
    pub fn find_rt(&self, name: &str) -> Option<Sym> {
        self.kinds.iter().position(|k| matches!(k, SymKind::Rt(n) if *n == name)).map(|i| Sym::new(i as u32))
    }

    /// A resolved C symbol, including symbols from libraries with local visibility.
    pub fn find_foreign(&self, name: &str) -> Option<Sym> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map.get(&SymKind::Foreign(name.to_string())).copied().map(Sym::new)
    }

    pub fn foreign_def(&self, id: u32) -> Option<DefId> {
        self.foreign_defs.lock().unwrap_or_else(|e| e.into_inner()).get(&id).copied()
    }

    /// The symbol a pointer value stands for (data object + offset, thunk, static), if any.
    pub fn symbol_at(&self, v: u64, u: &Unit) -> Option<Sym> {
        provenance(u, v)
    }

    /// (start, len, align, mutable) of data object `idx`.
    pub fn obj_info(&self, idx: u32) -> Option<(u64, u64, u64, bool)> {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        d.objs.get(idx as usize).map(|o| (o.start, o.len, o.align, o.mutable))
    }

    /// The stable identity of data object `idx` (const path + args, vtable types, location), if
    /// it has one (literals: none, they are one object per content).
    pub fn obj_name(&self, idx: u32) -> Option<String> {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        d.objs.get(idx as usize).and_then(|o| o.name.clone())
    }

    /// The data object a symbol names: (index, start, len).
    fn obj_of(&self, s: Sym) -> Option<u32> {
        match self.kind(s.id)? {
            SymKind::Data(i) => Some(i),
            _ => None,
        }
    }

    /// The object containing `addr` (start <= addr <= end: one past the end belongs to the
    /// object; `Unit::data` leaves a gap after every object, so that is never another start).
    /// The data objects the 8-byte words of `b` (those above 0x10000) point into, as
    /// `containing` finds them, under one lock.
    pub(crate) fn data_words(&self, b: &[u8]) -> Vec<Sym> {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        b.chunks_exact(8).filter_map(|w| {
            let addr = u64::from_le_bytes(w.try_into().unwrap());
            if addr <= 0x10000 {
                return None;
            }
            let (&start, &i) = d.by_start.range(..=addr).next_back()?;
            let o = &d.objs[i as usize];
            (addr <= o.start + o.len).then(|| Sym { id: o.sym, off: (addr - start) as i64 })
        }).collect()
    }

    fn containing(&self, addr: u64) -> Option<Sym> {
        let d = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let (&start, &i) = d.by_start.range(..=addr).next_back()?;
        let o = &d.objs[i as usize];
        if addr > o.start + o.len {
            return None;
        }
        Some(Sym { id: o.sym, off: (addr - start) as i64 })
    }
}

static UNIT_PTR: AtomicPtr<Unit> = AtomicPtr::new(std::ptr::null_mut());

pub fn set_unit(u: *mut Unit) {
    UNIT_PTR.store(u, Ordering::SeqCst);
}

/// The optimiser's constant-load reader (`opt::RoReader`): `n` bytes at symbol + offset of
/// the unit `ctx` points to -- the unit the function is built for (Unit::ro_reader), never a
/// process-wide one: the proc-macro world's unit is made after the program's on a warm start,
/// and folding the program's loads against its symbol tables (or, once the world is freed,
/// freed memory) baked another object's bytes into the program's code (a constant slice's
/// length read as 0; that O2 code went to the disk cache, so every later warm start ran it).
pub fn ro_read(ctx: usize, s: Sym, n: u32) -> crate::opt::RoRead {
    let u = unsafe { &*(ctx as *const Unit) };
    read_obj(u, s, n)
}

pub fn read_obj(u: &Unit, s: Sym, n: u32) -> crate::opt::RoRead {
    use crate::opt::RoRead;
    let idx = match u.syms.obj_of(s) {
        Some(i) => i,
        None => return RoRead::No,
    };
    if n == 0 {
        return match u.syms.obj_info(idx) {
            Some((_, len, _, false)) if s.off >= 0 && s.off as u64 <= len => RoRead::Range(len - s.off as u64),
            _ => RoRead::No,
        };
    }
    let (start, len, ptrs, opaque) = match obj_ptrs(u, idx) {
        Some(x) => x,
        None => return RoRead::No,
    };
    if opaque || s.off < 0 || s.off as u64 + n as u64 > len {
        return RoRead::No;
    }
    let off = s.off as u32;
    for (po, target) in ptrs.iter() {
        // any overlap with a pointer word: only the exact 8-byte read of it folds (to a symbol)
        if *po < off + n && off < po + 8 {
            return match target {
                Some(t) if *po == off && n == 8 => RoRead::Ptr(*t),
                _ => RoRead::No,
            };
        }
    }
    let p = (start + off as u64) as *const u8;
    let raw = unsafe {
        match n {
            1 => *p as u64,
            2 => (p as *const u16).read_unaligned() as u64,
            4 => (p as *const u32).read_unaligned() as u64,
            8 => (p as *const u64).read_unaligned(),
            _ => return RoRead::No,
        }
    };
    RoRead::Bytes(raw)
}

/// A constant leaf the front end reads at compile time (lower.rs const_val and pattern tests).
pub fn read_leaf(u: &Unit, s: Sym, m: Mem) -> crate::opt::RoRead {
    let n = match m {
        Mem::Int(n, _) => n as u32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    };
    read_obj(u, s, n)
}

/// The functions a constant data object at `addr` points at, directly or through the data
/// objects it points at (vtables and their supertrait vtables, fn tables, constants holding fn
/// pointers): what code reading it can call (real-time reach, jit.rs realtime).
pub(crate) fn data_thunks(u: &Unit, addr: u64, out: &mut Vec<u32>) {
    let mut seen = Vec::new();
    let mut todo: Vec<u32> = u.syms.data_at(addr).into_iter().filter_map(|s| match u.syms.kind(s.id) { Some(SymKind::Data(i)) => Some(i), _ => None }).collect();
    while let Some(idx) = todo.pop() {
        if seen.contains(&idx) || seen.len() > 4096 {
            continue;
        }
        seen.push(idx);
        let Some((_, _, ptrs, _)) = obj_ptrs(u, idx) else { continue };
        for (_, s) in ptrs.iter() {
            match s.and_then(|s| u.syms.kind(s.id)) {
                Some(SymKind::Thunk(id)) => out.push(id),
                Some(SymKind::Data(i)) => todo.push(i),
                _ => {}
            }
        }
    }
}

/// (start, len, pointer words, opaque) of object `idx`, computing the pointer words once.
fn obj_ptrs(u: &Unit, idx: u32) -> Option<(u64, u64, std::sync::Arc<Vec<(u32, Option<Sym>)>>, bool)> {
    let (start, len, kind) = {
        let d = u.syms.data.lock().unwrap_or_else(|e| e.into_inner());
        let o = d.objs.get(idx as usize)?;
        if o.mutable {
            return None;
        }
        if let Some(p) = &o.ptrs {
            return Some((o.start, o.len, p.clone(), o.opaque));
        }
        (o.start, o.len, o.kind.clone())
    };
    let mut ptrs = Vec::new();
    let mut opaque = false;
    match &kind {
        DataKind::Bytes => {}
        DataKind::Relocs(p) => ptrs.extend(p.iter().map(|(o, s)| (*o, Some(*s)))),
        DataKind::Words(w) => {
            // fewer words than the object holds: its pointer words are unknown (a cached object
            // whose pointers could not be attributed): never stored or folded
            if (w.len() as u64) < len / 8 {
                opaque = true;
            }
            for (i, x) in w.iter().enumerate() {
                if let Some(t) = x {
                    ptrs.push(((i * 8) as u32, Some(*t)));
                }
            }
        }
        DataKind::Typed(t) => {
            let mut words = Vec::new();
            if walk(u, *t, start, 0, &mut words, 0).is_none() {
                opaque = true;
            }
            for (off, pointee) in words {
                let v = unsafe { ((start + off as u64) as *const u64).read_unaligned() };
                // Null/dangling pointers and negative ABI sentinels (macOS RTLD_DEFAULT is
                // -2) live in the reserved end pages, never in a user allocation. They are
                // fixed integers, not addresses to relocate. Do not infer this for other
                // unattributed values: those may be heap or runtime-only addresses.
                if v < 4096 || v >= u64::MAX - 4095 {
                    continue;
                }
                // a word into an anonymous writable block names, in constant data, a read-only
                // copy of the block's bytes (promoted_copy) or nothing: the block's symbol would
                // be stored as a fresh, zeroed block per link (cache.rs enc_sym tag 8), so the
                // next run read zeros where the cold run read the initializer's bytes
                let s = match provenance(u, v) {
                    Some(s) if matches!(u.syms.kind(s.id), Some(SymKind::Rw(_))) => pointee.and_then(|m| promoted_copy(u, m, v)),
                    s => s.or_else(|| pointee.and_then(|m| promoted_copy(u, m, v))),
                };
                ptrs.push((off, s));
            }
        }
    }
    let ptrs = crate::zone_alloc::kept_with(|| std::sync::Arc::new(ptrs.clone()));
    let mut d = u.syms.data.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(o) = d.objs.get_mut(idx as usize) {
        o.ptrs = Some(ptrs.clone());
        o.opaque = opaque;
    }
    Some((start, len, ptrs, opaque))
}

/// A constant value's shared pointer into a writable block an initializer's code wrote (a
/// promoted temporary that was not constant bytes when the code was lowered, e.g. a table of
/// pointers: lower.rs `rw_sym`): the block's bytes as a read-only data object of the block's
/// type, and the pointer as that object plus its offset. A shared `&'static T` / `*const T`
/// into a `Freeze` block is never written through, so the copy is the same value (only its
/// address differs, as a promoted constant's may); `&mut`, `*mut`, interior mutability and
/// blocks linked from the disk cache (untyped) keep the pointer unattributed.
fn promoted_copy(u: &Unit, mutable: bool, v: u64) -> Option<Sym> {
    if mutable {
        return None;
    }
    let (start, size, ty) = {
        let b = u.syms.rw_starts.lock().unwrap_or_else(|e| e.into_inner());
        let (&start, &(size, ty)) = b.range(..=v).next_back()?;
        (start, size, ty)
    };
    if v >= start + size || !u.in_rw(start, size) || !u.tcx.is_freeze(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), ty) {
        return None;
    }
    // a constant's per-domain copy (domain.rs rehome_const) is per-domain on purpose: its words
    // reach statics, and a shared read-only copy would hand every domain the compile domain's
    let block = u.syms.rw_syms.lock().unwrap_or_else(|e| e.into_inner()).get(&start).copied();
    if block.is_some_and(|b| u.syms.const_copies.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&b.id)) {
        return None;
    }
    let l = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, ty);
    if l.size as u64 > size {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(start as *const u8, l.size as usize) }.to_vec();
    let a = u.data(&bytes, l.align.max(1) as u64, DataKind::Typed(ty));
    let mut s = u.data_sym(a);
    s.off = (v - start) as i64;
    Some(s)
}

/// What a pointer value stored in data points to, as a symbol: a data object (+ offset), a
/// function's thunk, a static. None: not attributable (the word then never folds).
fn provenance(u: &Unit, v: u64) -> Option<Sym> {
    if let Some(s) = u.syms.containing(v) {
        return Some(s);
    }
    if let Some(id) = u.thunk_slot(v) {
        return Some(u.syms.intern(SymKind::Thunk(id), v));
    }
    if let Some(s) = u.static_sym_at(v) {
        return Some(s);
    }
    // an anonymous mutable block (a promoted temporary, a per-domain constant): per-domain storage
    // like a static, so a word pointing into it is rebased in every domain's copy (domain.rs)
    if let Some(s) = u.syms.rw_sym_at(v) {
        return Some(s);
    }
    // Runtime entries are all installed at Unit creation. Prefer their stable names over
    // foreign aliases (e.g. memcpy), never the symbol id / order of compilation.
    let rt = u.syms.rt_by_addr.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        let mut entries: Vec<_> = u.syms.kinds.iter().enumerate().filter_map(|(i, k)| match k {
            SymKind::Rt(n) => Some((*n, Sym::new(i as u32))),
            _ => None,
        }).collect();
        entries.sort_unstable_by_key(|(n, s)| (*n, s.id));
        let mut by_addr = HashMap::new();
        for (_, s) in entries {
            by_addr.entry(u.syms.value(s)).or_insert(s);
        }
        by_addr
    });
    if let Some(&s) = rt.get(&v) {
        return Some(s);
    }
    // Search declarations, not just symbols encountered so far: the latter set depends on
    // compilation order. Distinct C names can alias one address; choose by link name, and
    // let enc_sym canonicalize duplicate declarations of that name. Resolve each batch only
    // once: misses must not trigger another declaration scan / dlsym per pointer word.
    let mut index = u.syms.foreign_by_addr.lock().unwrap_or_else(|e| e.into_inner());
    if index.defs_scanned < u.prog.defs.len() {
        let mut foreign: Vec<_> = u.prog.defs[index.defs_scanned..].iter().enumerate().filter_map(|(i, d)| {
            if matches!(d.kind, crate::program::DefKind::ForeignFn | crate::program::DefKind::ForeignStatic) {
                let d = DefId((index.defs_scanned + i) as u32);
                Some((u.link_name(d), d))
            } else {
                None
            }
        }).collect();
        foreign.sort_unstable_by(|(a, da), (b, db)| a.cmp(b).then(da.0.cmp(&db.0)));
        foreign.dedup_by(|a, b| a.0 == b.0);
        let _k = crate::zone_alloc::kept();
        for (_, d) in foreign {
            if let Some(addr) = u.loaded_foreign_addr(d) {
                // First per address wins, also when later modules add aliases.
                if let std::collections::hash_map::Entry::Vacant(e) = index.by_addr.entry(addr) {
                    if let Some(s) = u.foreign_sym(d) {
                        e.insert(s);
                    }
                }
            }
        }
        index.defs_scanned = u.prog.defs.len();
    }
    index.by_addr.get(&v).copied()
}

#[cfg(test)]
mod provenance_tests {
    use super::*;

    #[test]
    fn address_maps_are_lazy_and_extend_for_new_defs() {
        isolated_test!(address_maps_are_lazy_and_extend_for_new_defs);
        let sess = &std::sync::Arc::new(crate::Session::default());
        let root = std::env::temp_dir().join(format!("relax-provenance-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("first.rs");
        std::fs::write(&source, r#"
            extern "C" {
                #[link_name = "abs"] fn abs_b(x: i32) -> i32;
                #[link_name = "abs"] fn abs_a(x: i32) -> i32;
            }
        "#).unwrap();
        let mut prog = crate::program::Program::new(sess);
        prog.load_crate("first", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let mut u = Unit::new(prog, tcx, 0);
        let def = |u: &Unit, name: &str| DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        assert!(u.syms.rt_by_addr.get().is_none());
        assert_eq!(u.syms.foreign_by_addr.lock().unwrap().defs_scanned, 0);
        let data = u.data(b"data", 1, DataKind::Bytes);
        assert_eq!(provenance(&u, data), Some(u.data_sym(data)));
        assert!(u.syms.rt_by_addr.get().is_none());

        let rt = u.syms.find_rt("memcpy").unwrap();
        assert_eq!(provenance(&u, u.syms.value(rt)), Some(rt));
        assert!(u.syms.rt_by_addr.get().is_some());
        assert_eq!(u.syms.foreign_by_addr.lock().unwrap().defs_scanned, 0);

        let abs = def(&u, "abs_b");
        let addr = u.loaded_foreign_addr(abs).unwrap();
        let sym = provenance(&u, addr).unwrap();
        // Equal link names choose the first DefId, regardless of Rust declaration name.
        assert_eq!(u.syms.foreign_def(sym.id), Some(abs));
        assert_eq!(u.syms.foreign_by_addr.lock().unwrap().defs_scanned, u.prog.defs.len());
        let heap = Box::new(42u64);
        for _ in 0..100 {
            assert_eq!(provenance(&u, addr), Some(sym));
            assert_eq!(provenance(&u, &*heap as *const u64 as u64), None);
        }

        let source = root.join("later.rs");
        std::fs::write(&source, r#"
            extern "C" {
                fn labs(x: i64) -> i64;
                #[link_name = "abs"] fn abs_later(x: i32) -> i32;
            }
        "#).unwrap();
        u.prog.load_crate("later", source.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(sess, &[]), Vec::new()).unwrap();
        let labs = def(&u, "labs");
        let addr_labs = u.loaded_foreign_addr(labs).unwrap();
        let sym_labs = provenance(&u, addr_labs).unwrap();
        assert_eq!(u.syms.foreign_def(sym_labs.id), Some(labs));
        assert_eq!(provenance(&u, addr), Some(sym));
        assert_eq!(u.syms.foreign_by_addr.lock().unwrap().defs_scanned, u.prog.defs.len());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Collects the pointer words of a value of type `t` at `base + off`: (offset, whether a data
/// pointer is `&mut` / `*mut`; None for a fn pointer or a vtable word). None: the value cannot
/// be classified (union, unknown layout): the whole object is opaque.
fn walk(u: &Unit, t: TyId, base: u64, off: u32, out: &mut Vec<(u32, Option<bool>)>, depth: u32) -> Option<()> {
    if depth > 24 {
        return None;
    }
    // N:rustc_ty_utils/src/layout.rs:63-75 @8d1a764: inspect the normalized
    // type, including native array lengths, just as the layout query does.
    let t = u.tcx.try_normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), t).ok()?;
    let rd = |o: u32| unsafe { ((base + o as u64) as *const u64).read_unaligned() };
    match u.tcx.tys.kind(t).clone() {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never => Some(()),
        TyKind::Tuple(ts) if ts.is_empty() => Some(()),
        TyKind::Ref(m, inner, _) | TyKind::Ptr(m, inner) => {
            out.push((off, Some(m)));
            match u.tcx.tys.kind(inner).clone() {
                // fat pointers: (ptr, len) or (ptr, vtable)
                TyKind::Str | TyKind::Slice(_) => {}
                TyKind::Dyn(..) => out.push((off + 8, None)),
                // an unsized struct (`&CStr`, `&Path`): its tail's metadata, a length or a vtable
                _ => match u.lay.unsized_tail(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, inner) {
                    Some((_, tail)) => match u.tcx.tys.kind(tail) {
                        TyKind::Str | TyKind::Slice(_) => {}
                        TyKind::Dyn(..) => out.push((off + 8, None)),
                        _ => return None,
                    },
                    None => {}
                },
            }
            let _ = rd;
            Some(())
        }
        TyKind::FnPtr(..) => {
            out.push((off, None));
            Some(())
        }
        TyKind::FnDef(..) => Some(()),
        TyKind::Array(et, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys).expect("normalized array length");
            let es = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, et).size as u64;
            if es == 0 {
                return Some(());
            }
            let n = n.min(1 << 20);
            // a scalar element type without pointers: nothing to walk per element
            let mut probe = Vec::new();
            walk(u, et, base, off, &mut probe, depth + 1)?;
            if probe.is_empty() && !has_enum(u, et, 0) {
                return Some(());
            }
            for i in 0..n {
                walk(u, et, base, off + (i * es) as u32, out, depth + 1)?;
            }
            Some(())
        }
        TyKind::Tuple(ts) => {
            let l = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t);
            for (i, ft) in u.tcx.tys.list(ts).iter().enumerate() {
                let fo = *l.fields.get(i)?;
                walk(u, *ft, base, off + fo, out, depth + 1)?;
            }
            Some(())
        }
        TyKind::Adt(d, args, _) => {
            let adt = u.tcx.adts.get(&d)?.clone();
            if adt.is_union {
                return None;
            }
            let l = u.lay.of(&crate::layout::LayoutCx::new(&u.prog), &u.tcx, t);
            let vi = if adt.is_enum {
                match l.tag {
                    Some((toff, Mem::Int(n, _))) => {
                        let mut raw = [0u8; 16];
                        let p = (base + (off + toff) as u64) as *const u8;
                        for k in 0..n as usize {
                            raw[k] = unsafe { *p.add(k) };
                        }
                        let disc = crate::lower::decode_tag_const(u, i128::from_le_bytes(raw), &l, &adt)?;
                        adt.discriminants(&u.tcx, &u.prog).iter().position(|(_, v)| v.signed_value() == disc)?
                    }
                    Some(_) => return None,
                    None => 0,
                }
            } else {
                0
            };
            let fields = adt.variants.get(vi)?.fields.clone();
            for fi in 0..fields.len() {
                let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args));
                let fo = if adt.is_enum { l.variant_fields.get(vi).and_then(|v| v.get(fi)).copied() } else { l.fields.get(fi).copied() };
                walk(u, ft, base, off + fo?, out, depth + 1)?;
            }
            Some(())
        }
        // closures, coroutines, opaque types, trait objects by value: not classified
        _ => None,
    }
}

/// Whether a type contains an enum (its pointer words then depend on the value).
fn has_enum(u: &Unit, t: TyId, depth: u32) -> bool {
    if depth > 8 {
        return true;
    }
    match u.tcx.tys.kind(t).clone() {
        TyKind::Adt(d, args, _) => match u.tcx.adts.get(&d) {
            Some(a) if a.is_enum => true,
            Some(a) => a.variants.iter().enumerate().any(|(vi, v)| (0..v.fields.len()).any(|fi| has_enum(u, u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args)), depth + 1))),
            None => true,
        },
        TyKind::Tuple(ts) => u.tcx.tys.list(ts).iter().any(|x| has_enum(u, *x, depth + 1)),
        TyKind::Array(e, _) => has_enum(u, e, depth + 1),
        _ => false,
    }
}

/// Every symbol and its kind (MAKEPAD_RUN_SYMS diagnostics).
pub fn dump(u: &Unit) -> Vec<(u32, SymKind, u64)> {
    u.syms.kinds.iter().enumerate().map(|(i, k)| (i as u32, k.clone(), unsafe { std::ptr::read((u.syms.vals as *const u64).add(i)) })).collect()
}

/// A stable description of a symbol (the same in every run of the same sources): what the
/// MAKEPAD_RUN_RELOC_SIG / MAKEPAD_RUN_CODE_SIG dumps print instead of symbol ids, and what the disk cache
/// will key relocations by.
pub fn describe(u: &Unit, s: Sym) -> String {
    let base = match u.syms.kind(s.id) {
        None => format!("?{}", s.id),
        Some(SymKind::Rt(n)) => format!("rt:{}", n),
        Some(SymKind::Static(d)) => format!("static:{}", u.prog.def_path(d)),
        Some(SymKind::Foreign(n)) => format!("c:{}", n),
        Some(SymKind::Missing(n)) => format!("missing:{}", n),
        Some(SymKind::Asm(t)) => format!("asm:{:032x}", crate::sip::hash_bytes(t.as_bytes())),
        Some(SymKind::Site(i)) => {
            let sites = u.sites.lock().unwrap_or_else(|e| e.into_inner());
            match sites.get(i as usize) {
                Some(x) => format!("site:{}:{}:{}", u.prog.file_paths.get(x.file as usize).map_or("", |p| p.as_str()), x.pos, x.msg),
                None => format!("site:?{}", i),
            }
        }
        Some(SymKind::Thunk(id)) => format!("thunk:{}", stable_fn_name(u, id)),
        Some(SymKind::Provided(n)) => format!("provided:{}", n),
        Some(SymKind::Rw(k)) => {
            let b = u.syms.rw_blocks.lock().unwrap_or_else(|e| e.into_inner());
            let (sz, al) = b.get(k as usize).copied().unwrap_or((0, 0));
            format!("rw:{}:{}", sz, al)
        }
        Some(SymKind::Data(i)) => {
            let (name, start, len, align, mutable) = {
                let d = u.syms.data.lock().unwrap_or_else(|e| e.into_inner());
                match d.objs.get(i as usize) {
                    Some(o) => (o.name.clone(), o.start, o.len, o.align, o.mutable),
                    None => return format!("data:?{}", i),
                }
            };
            match name {
                Some(n) => format!("data:{}", n),
                // a writable block's bytes are the program's (pointers included): by shape
                None if mutable => format!("rwobj:{}:{}", len, align),
                None => {
                    // pointer words by what they point to, not by this run's addresses
                    let mut b = unsafe { std::slice::from_raw_parts(start as *const u8, len as usize) }.to_vec();
                    let mut h = crate::sip::Sip128::new();
                    for (o, t) in obj_pointer_words(u, i).unwrap_or_default() {
                        let o = o as usize;
                        if o + 8 <= b.len() {
                            b[o..o + 8].fill(0);
                        }
                        h.u32(o as u32);
                        h.str(&describe(u, t));
                    }
                    h.write(&b);
                    format!("bytes:{:032x}", h.finish128())
                }
            }
        }
    };
    if s.off == 0 {
        base
    } else {
        format!("{}{:+}", base, s.off)
    }
}

/// `Sym { id: N, off: M }` in a RIR dump -> its stable description.
pub fn canon_dump(u: &Unit, d: &str) -> String {
    let mut out = String::with_capacity(d.len());
    let mut rest = d;
    while let Some(i) = rest.find("Sym { id: ") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 10..];
        let parsed = (|| {
            let c = tail.find(", off: ")?;
            let id: u32 = tail[..c].parse().ok()?;
            let t2 = &tail[c + 7..];
            let e = t2.find(" }")?;
            let off: i64 = t2[..e].parse().ok()?;
            Some((Sym { id, off }, c + 7 + e + 2))
        })();
        match parsed {
            Some((sym, n)) => {
                out.push_str("Sym(");
                out.push_str(&describe(u, sym));
                out.push(')');
                rest = &tail[n..];
            }
            None => {
                out.push_str("Sym { id: ");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// MAKEPAD_RUN_RELOC_SIG=<file>: per compiled function, a hash of its machine code with the address
/// pool zeroed, its relocations by stable description and its pc map by file path. Two runs of
/// the same program (one with MAKEPAD_RUN_PERTURB) must print the same lines: a difference is a
/// run-dependent value that reached code without a relocation.
pub fn reloc_sig(u: &Unit, name: impl FnOnce() -> String, code: &[u8], relocs: &[crate::rir::RelocAt], pc_map: &[(u32, u64)]) {
    static F: std::sync::OnceLock<Option<Mutex<std::fs::File>>> = std::sync::OnceLock::new();
    let m = match F.get_or_init(|| std::env::var("MAKEPAD_RUN_RELOC_SIG").ok().and_then(|p| std::fs::File::create(p).ok()).map(Mutex::new)) {
        Some(m) => m,
        None => return,
    };
    let name = name();
    let mut c = code.to_vec();
    let mut rs: Vec<String> = Vec::new();
    for rl in relocs {
        let (off, r) = (&rl.at, &rl.target);
        let o = *off as usize;
        if o + 8 <= c.len() {
            c[o..o + 8].fill(0);
        }
        let d = match r {
            crate::rir::Reloc::Sym(s) => describe(u, *s),
            crate::rir::Reloc::Slot(id) => format!("slot:{}", stable_fn_name(u, *id)),
            crate::rir::Reloc::Thunk(id) => format!("thunk:{}", stable_fn_name(u, *id)),
            other => format!("{:?}", other),
        };
        rs.push(format!("{}={}", off, d));
    }
    let mut h = crate::sip::Sip128::new();
    h.write(&c);
    for x in &rs {
        h.str(x);
    }
    for (o, pos) in pc_map {
        h.u32(*o);
        h.str(u.prog.file_paths.get((pos >> 32) as usize).map_or("", |p| p.as_str()));
        h.u32(*pos as u32);
    }
    use std::io::Write;
    if let Ok(want) = std::env::var("MAKEPAD_RUN_RELOC_SIG_CODE") {
        if want == "*" || name.contains(want.as_str()) {
            let hex: String = c.iter().map(|b| format!("{:02x}", b)).collect();
            // the pool as compiled (values), for debugging relocations
            let vals: Vec<String> = relocs.iter().map(|rl| {
                let (o, r) = (&rl.at, &rl.target);
                let o = *o as usize;
                let v = if o + 8 <= code.len() { u64::from_le_bytes(code[o..o + 8].try_into().unwrap()) } else { 0 };
                format!("{}:{:?}={:#x}", o, r, v)
            }).collect();
            let _ = m.lock().unwrap_or_else(|e| e.into_inner()).write_all(format!("POOL\t{}\t{}\n", name, vals.join(" ")).as_bytes());
            let pcs: Vec<String> = pc_map.iter().map(|(o, p)| format!("{}:{}:{}", o, p >> 32, *p as u32)).collect();
            let _ = m.lock().unwrap_or_else(|e| e.into_inner()).write_all(format!("CODE\t{}\t{}\t{}\n", name, hex, pcs.join(" ")).as_bytes());
        }
    }
    let line = if std::env::var_os("MAKEPAD_RUN_RELOC_SIG_VERBOSE").is_some() {
        format!("{}\t{}\t{:032x}\t{}\n", name, code.len(), h.finish128(), rs.join(" "))
    } else {
        format!("{}\t{}\t{:032x}\n", name, code.len(), h.finish128())
    };
    let _ = m.lock().unwrap_or_else(|e| e.into_inner()).write_all(line.as_bytes());
}

/// MAKEPAD_RUN_PERTURB=<n>: shifts the per-run numbering compiled code must not depend on (dummy
/// sites and symbols; the data / static areas start later). 0 when unset.
pub fn perturb(options: &crate::options::Options) -> u64 {
    options.perturb
}

/// A slot's name without slot numbers (a closure is named by its file, expression and its
/// parent's stable name, not the parent's slot id).
pub fn stable_fn_name(u: &Unit, id: u32) -> String {
    stable_fn_name_d(u, id, 0)
}

fn stable_fn_name_d(u: &Unit, id: u32, depth: u32) -> String {
    if id as usize >= u.fns.len() {
        return format!("?{}", id);
    }
    let f = &u.fns[id as usize];
    match f.key.clone() {
        crate::jit::FnKey::Inst(d, a) => {
            let mut s = u.prog.def_path(d);
            for t in a.iter() {
                s.push(',');
                s.push_str(&stable_ty(u, *t));
            }
            s
        }
        crate::jit::FnKey::Sandbox(b) => if b == u32::MAX { "sandbox:invalid".to_string() } else { format!("sandbox:{}", stable_fn_name_d(u, b, depth + 1)) },
        crate::jit::FnKey::Closure(file, e, p) => {
            let parent = if depth < 16 { stable_fn_name_d(u, p, depth + 1) } else { String::new() };
            format!("closure:{}:{}@{}", u.prog.file_paths.get(file as usize).map_or("", |x| x.as_str()), e, parent)
        }
        crate::jit::FnKey::Shim(ref shim) => crate::lower::shim_name(&u.prog, &u.tcx, shim),
        crate::jit::FnKey::Glue(g, t) => format!("glue{}:{}", g, stable_ty(u, t)),
    }
}

/// The program's unit (for diagnostics that have no `&Unit` at hand; run.rs set_unit).
pub fn unit() -> Option<&'static Unit> {
    let up = UNIT_PTR.load(Ordering::Acquire);
    if up.is_null() {
        None
    } else {
        Some(unsafe { &*up })
    }
}

/// A type's printed name with a short stable hash (two types that print alike stay apart).
pub fn stable_ty(u: &Unit, t: TyId) -> String {
    let t = u.tcx.tys.erase_regions(t);
    format!("{}#{:08x}", crate::typeck::ty_to_string(false, &u.prog, &u.tcx, t), crate::stable::type_hash(&u.prog, &u.tcx, t) as u32)
}

/// The pointer words of data object `idx` with their targets; None when the object cannot be
/// described (mutable, opaque, a pointer whose target has no symbol): the disk cache then
/// cannot store code that refers to it.
/// The offsets of the pointer words of the data object starting at `start`, from what its
/// bytes are (its type, its relocations, its word table) -- never from what the words happen to
/// hold: a table of integers can hold a value that is an address in this run. None when the
/// object is not one, or its kind cannot tell (a union, a word table shorter than the object).
pub(crate) fn pointer_offsets(u: &Unit, start: u64) -> Option<Vec<u32>> {
    let (len, kind) = {
        let d = u.syms.data.lock().unwrap_or_else(|e| e.into_inner());
        let &i = d.by_start.get(&start)?;
        let o = &d.objs[i as usize];
        (o.len, o.kind.clone())
    };
    match kind {
        DataKind::Bytes => Some(Vec::new()),
        DataKind::Relocs(p) => Some(p.iter().map(|(o, _)| *o).collect()),
        DataKind::Words(w) => ((w.len() as u64) >= len / 8).then(|| w.iter().enumerate().filter(|(_, x)| x.is_some()).map(|(i, _)| (i * 8) as u32).collect()),
        DataKind::Typed(t) => {
            let mut words = Vec::new();
            walk(u, t, start, 0, &mut words, 0)?;
            Some(words.into_iter().map(|(o, _)| o).collect())
        }
    }
}

pub fn obj_pointer_words(u: &Unit, idx: u32) -> Option<Vec<(u32, Sym)>> {
    let (_, _, ptrs, opaque) = obj_ptrs(u, idx)?;
    if opaque {
        return None;
    }
    let mut out = Vec::with_capacity(ptrs.len());
    for (o, t) in ptrs.iter() {
        out.push((*o, (*t)?));
    }
    Some(out)
}

/// How code may refer to a symbol of this kind: the same in every run (it is part of the code's
/// shape). Near = inside the unit's reservation (pc-relative), Far = anywhere (pool word), Imm =
/// a per-run number (immediate field).
pub fn class_of(k: &SymKind) -> crate::rir::SymClass {
    use crate::rir::SymClass;
    match k {
        // glue emitted into the unit's code area; the rest are host functions
        SymKind::Rt(n) => match *n {
            "panic_site" | "panic_bounds" | "panic_impl" | "resume" | "land" => SymClass::Near,
            _ => SymClass::Far,
        },
        // storage every domain has its own copy of, at the same offset of its static block
        SymKind::Static(_) | SymKind::Rw(_) => SymClass::Domain,
        SymKind::Data(_) | SymKind::Thunk(_) | SymKind::Asm(_) | SymKind::Missing(_) => SymClass::Near,
        SymKind::Foreign(_) | SymKind::Provided(_) => SymClass::Far,
        SymKind::Site(_) => SymClass::Imm,
    }
}

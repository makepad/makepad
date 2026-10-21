//! The kept-state scan (`MAKEPAD_RUN_ZONE_CHECK=1`): a walk over the closed list of Unit state that
//! must survive the destruction of the compiler arena (memlane step 1b), counting every owned
//! heap allocation that lies inside that arena. A count above zero is a site that built kept
//! state in the compiler phase (it needs `zone_alloc::kept()` around the allocation).
//!
//! The list is the K class of the Unit (jit.rs, `Unit`): the compile state (C) is not walked.

use super::*;
use std::collections::{BTreeMap, HashSet};

/// Old compiler statics, owned by the swap thread until rollback or commit.
pub(crate) struct ZoneTaken {
    compiler: crate::ZoneTaken,
    types: relax_types::ZoneTaken,
    front: relax_front::ZoneTaken,
    codegen: relax_codegen::ZoneTaken,
    session: relax_session::ZoneTaken,
}

impl ZoneTaken {
    /// Restores the running generation's statics and forgets the aborted build's values.
    pub(crate) fn put_back(self) {
        crate::zone_put_back(self.compiler);
        relax_types::zone_put_back(self.types);
        relax_front::zone_put_back(self.front);
        relax_codegen::zone_put_back(self.codegen);
        relax_session::zone_put_back(self.session);
    }

    /// The proc-macro registry of the running program, registered again in the new generation
    /// (the taken statics stay as they are, for rollback).
    pub(crate) fn reinstall_macro_registry(&self, sess: &std::sync::Arc<crate::Session>) -> Result<(), String> {
        self.front.reinstall_macro_registry(sess)
    }

    /// The old values may point into any older generation; never drop them at commit.
    pub(crate) fn forget(self) {
        std::mem::forget(self);
    }
}

/// Takes compiler statics before a restore, keeping rollback possible and compiler TLS valid.
/// Every compiler thread is quiesced; the caller is already in the new compiler generation.
pub(crate) fn zone_take_all() -> Option<ZoneTaken> {
    Some(ZoneTaken {
        compiler: crate::zone_take()?,
        types: relax_types::zone_take(),
        front: relax_front::zone_take(),
        codegen: relax_codegen::zone_take(),
        session: relax_session::zone_take(),
    })
}

#[cfg(test)]
mod reset_tests {
    #[test]
    fn zone_reset_fresh_process() {
        const CHILD: &str = "MAKEPAD_RUN_ZONE_RESET_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            // Take helpers assert their statics are empty. Repeat to cover an empty take.
            let _phase = crate::zone_alloc::compiler();
            super::zone_take_all().unwrap().forget();
            super::zone_take_all().unwrap().forget();
            return;
        }
        // Other native tests use these process-wide caches: reset only in an isolated child.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::zone_scan::reset_tests::zone_reset_fresh_process"])
            .env(CHILD, "1")
            .status().unwrap();
        assert!(status.success());
    }
}

/// Counts, per named field, the owned allocations that lie in the compiler arena.
pub(crate) struct Acc {
    counts: Vec<(&'static str, usize)>,
    cur: usize,
}

impl Acc {
    fn new() -> Acc {
        Acc { counts: Vec::new(), cur: 0 }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test() -> Acc {
        Acc::new()
    }

    /// Pointers into the compiler arena counted so far.
    #[cfg(test)]
    pub(crate) fn total(&self) -> usize {
        self.counts.iter().map(|c| c.1).sum()
    }

    /// The following pointers are counted under `name`.
    pub(crate) fn field(&mut self, name: &'static str) {
        self.cur = match self.counts.iter().position(|c| c.0 == name) {
            Some(i) => i,
            None => {
                self.counts.push((name, 0));
                self.counts.len() - 1
            }
        };
    }

    /// An allocation the state owns.
    pub(crate) fn ptr<T: ?Sized>(&mut self, p: *const T) {
        if crate::zone_alloc::compiler_owns(p as *const u8) {
            self.counts[self.cur].1 += 1;
        }
    }

    pub(crate) fn vec<T>(&mut self, v: &Vec<T>) {
        if v.capacity() > 0 && std::mem::size_of::<T>() > 0 {
            self.ptr(v.as_ptr());
        }
    }

    pub(crate) fn string(&mut self, s: &String) {
        if s.capacity() > 0 {
            self.ptr(s.as_ptr());
        }
    }

    pub(crate) fn boxed<T: ?Sized>(&mut self, b: &Box<T>) {
        if std::mem::size_of_val(&**b) > 0 {
            self.ptr(&**b as *const T);
        }
    }

    pub(crate) fn arc<T: ?Sized>(&mut self, a: &std::sync::Arc<T>) {
        self.ptr(std::sync::Arc::as_ptr(a));
    }

    /// A hash map: each entry lies in the table's allocation (an entry reference is an address
    /// in it), then what the entry owns.
    pub(crate) fn map<K, V, S>(&mut self, m: &std::collections::HashMap<K, V, S>, mut deep: impl FnMut(&mut Acc, &K, &V)) {
        for (k, v) in m.iter() {
            if std::mem::size_of::<(K, V)>() > 0 {
                self.ptr(k as *const K);
            }
            deep(self, k, v);
        }
    }

    pub(crate) fn set<T, S>(&mut self, m: &HashSet<T, S>) {
        for k in m.iter() {
            if std::mem::size_of::<T>() > 0 {
                self.ptr(k as *const T);
            }
        }
    }

    /// A B-tree map: each key lies in its node's allocation.
    pub(crate) fn btree<K, V>(&mut self, m: &BTreeMap<K, V>, mut deep: impl FnMut(&mut Acc, &K, &V)) {
        for (k, v) in m.iter() {
            if std::mem::size_of::<K>() > 0 {
                self.ptr(k as *const K);
            }
            deep(self, k, v);
        }
    }
}

impl PcMap {
    fn scan(&self, acc: &mut Acc) {
        match &self.0 {
            PcBytes::Own(b) => {
                if !b.is_empty() {
                    acc.ptr(b.as_ptr());
                }
            }
            PcBytes::Mapped(_, f) => acc.pc_files(f),
        }
    }
}

impl Acc {
    fn pc_files(&mut self, f: &std::sync::Arc<PcFiles>) {
        self.arc(f);
        self.vec(&f.files);
        self.ptr(&*f.keep as *const (dyn std::any::Any + Send + Sync));
    }

    fn fn_key(&mut self, k: &FnKey) {
        match k {
            FnKey::Inst(_, a) => self.vec(a),
            FnKey::Shim(s) => self.vec(&s.args),
            _ => {}
        }
    }
}

/// Owned allocations of the kept state that lie in the compiler arena, per field (only fields
/// with a nonzero count are listed). Quiescent callers only (the owner thread at a settle).
pub(crate) fn kept_scan(u: &Unit) -> Vec<(&'static str, usize)> {
    let mut a = Acc::new();
    a.field("unit");
    a.ptr(u as *const Unit);
    a.field("slots");
    a.boxed(&u.slots);
    if let Ok(c) = u.code.try_read() {
        a.field("code.index");
        a.vec(&c.index);
        a.field("code.unwind");
        a.map(&c.unwind, |a, _, v| {
            a.vec(&v.saved);
            a.vec(&v.pads);
        });
        a.field("code.image_unwind");
        a.vec(&c.image_unwind);
        if let Some(f) = &c.image_keep {
            a.pc_files(f);
        }
    }
    a.field("fns");
    u.fns.alloc_ptrs(|p| a.ptr(p));
    for f in u.fns.iter() {
        a.field("fn.key");
        a.fn_key(&f.key);
        a.field("fn.name");
        a.boxed(&f.name);
        a.field("fn.pc_map");
        f.pc_map.scan(&mut a);
        if let Some(h) = &f.history {
            a.field("fn.history");
            a.boxed(h);
            a.field("fn.prev");
            a.vec(&h.prev);
            for p in &h.prev {
                p.2.scan(&mut a);
            }
            a.field("fn.retired");
            a.vec(&h.retired);
            for p in &h.retired {
                p.2.scan(&mut a);
            }
            a.field("fn.inlined_into");
            a.vec(&h.inlined_into);
        }
    }
    a.field("fn_map");
    {
        let mut own = Vec::new();
        let mut keys = Vec::new();
        u.fn_map.scan(|p| own.push(p), |k, _| keys.push(k as *const FnKey));
        for p in own {
            a.ptr(p);
        }
        for k in keys {
            a.fn_key(unsafe { &*k });
        }
    }
    a.field("consts");
    a.map(&u.consts, |a, k, _| a.vec(&k.1));
    a.field("const_readers");
    if let Ok(g) = u.const_readers.try_lock() {
        a.map(&*g, |a, _, v| a.vec(v));
    }
    a.field("body_deps");
    a.map(&u.body_deps, |a, _, v| a.vec(v));
    a.field("statics");
    a.map(&u.statics, |_, _, _| {});
    a.field("static_spans");
    a.btree(&u.static_spans, |_, _, _| {});
    a.field("sites");
    if let Ok(g) = u.sites.try_lock() {
        a.vec(&g);
        for s in g.iter() {
            a.string(&s.msg);
        }
    }
    a.field("site_ix");
    if let Ok(g) = u.site_ix.try_lock() {
        a.map(&*g, |a, k, _| a.string(&k.2));
    }
    u.syms.scan(&mut a);
    a.field("start_order");
    a.vec(&u.start_order);
    a.field("vtables");
    {
        let mut own = Vec::new();
        u.vtables.scan(|p| own.push(p), |_, _| {});
        for p in own {
            a.ptr(p);
        }
    }
    a.field("locations");
    if let Ok(g) = u.locations.try_lock() {
        a.map(&*g, |a, k, _| a.string(&k.0));
    }
    a.field("lit_data");
    if let Ok(g) = u.lit_data.try_lock() {
        a.map(&*g, |a, k, _| a.vec(&k.0));
    }
    a.field("provided");
    a.vec(&u.provided);
    for (_, p) in &u.provided {
        a.string(&p.name);
        a.map(&p.shims, |a, k, _| a.string(k));
        a.map(&p.types, |a, k, _| a.string(k));
        a.map(&p.layouts, |a, k, _| a.string(k));
    }
    a.field("provided_types");
    a.map(&u.provided_types, |_, _, _| {});
    a.field("provided_shared");
    if let Some(s) = u.provided_shared_set() { a.set(s); }
    a.field("missing_stubs");
    if let Ok(g) = u.missing_stubs.try_lock() {
        a.map(&*g, |a, k, _| a.string(k));
    }
    a.field("patch_log");
    a.vec(&u.patch_log);
    a.field("cold_users");
    a.map(&u.cold_users, |a, _, v| a.vec(v));
    a.field("retiers");
    a.map(&u.retiers, |_, _, _| {});
    a.field("frequency_profiles");
    a.map(&u.frequency_profiles, |a, _, p| a.map(&p.positions, |_, _, _| {}));
    a.field("errors");
    a.vec(&u.errors);
    for e in &u.errors {
        a.string(e);
    }
    a.field("stats.timeline");
    a.vec(&u.stats.timeline);
    a.field("stats.tier1_drop_why");
    a.btree(&u.stats.tier1_drop_why, |_, _, _| {});
    // makepad inspect's records that installed code calls into (sites, probe groups, edits)
    crate::inspect::scan_kept(&mut a);
    a.counts.retain(|c| c.1 > 0);
    a.counts
}

/// `MAKEPAD_RUN_ZONE_CHECK`: 0 off, 1 the kept-state scan at every settle, 2 that plus the static scan
/// (writable data of the executable and the TLS of every thread, `static_scan`).
fn level() -> u8 {
    static L: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *L.get_or_init(|| match std::env::var("MAKEPAD_RUN_ZONE_CHECK").as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        _ => 0,
    })
}

/// Whether the kept-state scan runs (`MAKEPAD_RUN_ZONE_CHECK=1` or 2).
pub(crate) fn enabled() -> bool {
    level() >= 1
}

/// The scan's result: the one-line report, and the total (for `inspect memory`).
pub(crate) fn report(u: &Unit) -> (usize, String) {
    let counts = kept_scan(u);
    let total: usize = counts.iter().map(|c| c.1).sum();
    let list: Vec<String> = counts.iter().map(|(n, c)| format!("{n}: {c}")).collect();
    (total, format!("makepad-run zone: kept scan: {total} pointers into the compiler arena [{}]", list.join(", ")))
}

static LAST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

/// The total of the latest scan (None: the check is off or has not run).
pub(crate) fn last_total() -> Option<usize> {
    match LAST.load(std::sync::atomic::Ordering::Relaxed) {
        usize::MAX => None,
        n => Some(n),
    }
}

/// Runs the scan and prints its line (a settle's owner call; silent unless MAKEPAD_RUN_ZONE_CHECK=1).
pub(crate) fn settle_check(u: &Unit) {
    if !enabled() {
        return;
    }
    let (total, line) = report(u);
    LAST.store(total, std::sync::atomic::Ordering::Relaxed);
    eprintln!("{line}");
    if level() >= 2 {
        static_scan();
    }
}

// ------------------------------------------------------------------ static scan (level 2)
//
// Conservative scan of everything outside the Unit that can hold a pointer into the compiler
// arena of the current generation at a settle: the writable data of the executable (and of
// libmakepad when Relax is a library), and the static TLS of every thread. Each 8-byte-aligned
// word that points into a live block area of the arena is printed as `module+offset`, which
// `nm -C --defined-only` resolves to the static (statics) or the thread_local (`tls[exe]+offset`
// against `nm`'s TLS symbol values). It finds the roots a swap must reset, keep or forbid.

/// One scanned word: where it lives and what it holds.
struct Hit {
    place: String,
    target: usize,
}

fn print_hits(what: &str, hits: &[Hit], extra: &str) {
    eprintln!("makepad-run zone: {what}: {} words into the compiler arena{extra}", hits.len());
    // up to 200 entries; MAKEPAD_RUN_ZONE_SCAN_MAX raises it (the classification wants them all)
    let max = std::env::var("MAKEPAD_RUN_ZONE_SCAN_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(200usize);
    for h in hits.iter().take(max) {
        eprintln!("makepad-run zone:   {} -> {:#x}", h.place, h.target);
    }
}

/// The words of `[start, start+len)` that point into generation `g` (exact), skipping the
/// arena's own header table.
fn scan_words(start: usize, len: usize, g: u64, mut place: impl FnMut(usize) -> String, out: &mut Vec<Hit>) {
    let skip = crate::zone_alloc::compiler_internal_ranges();
    let mut a = (start + 7) & !7;
    let end = start + len;
    while a + 8 <= end {
        if !skip.iter().any(|r| a >= r.0 && a < r.1) {
            let v = unsafe { *(a as *const usize) };
            if crate::zone_alloc::compiler_owns(v as *const u8) && crate::zone_alloc::compiler_points_into(v, g) {
                out.push(Hit { place: place(a), target: v });
            }
        }
        a += 8;
    }
}

pub(crate) fn static_scan() {
    let g = crate::zone_alloc::compiler_generation();
    let mut hits = Vec::new();
    let mut words = 0usize;
    for (name, base, start, len) in data_regions() {
        words += len / 8;
        scan_words(start, len, g, |a| format!("{name}+{:#x}", a - base), &mut hits);
    }
    print_hits("static scan", &hits, &format!(" (generation {g}, {words} words scanned)"));
    tls_scan(g);
}

/// MAKEPAD_RUN_ZONE_CHECK=2, just before a swap destroys generation `old`: the statics already hold
/// the new generation's values, so a word still pointing into `old` is a root the swap missed (the
/// next use of it faults).
pub(crate) fn stale_roots(old: u64) {
    if level() < 2 {
        return;
    }
    let mut hits = Vec::new();
    for (name, base, start, len) in data_regions() {
        scan_words(start, len, old, |a| format!("{name}+{:#x}", a - base), &mut hits);
    }
    print_hits("STALE statics before destroy", &hits, &format!(" (generation {old})"));
}

/// (module, load base, start, len) of every writable data range of the executable and, when
/// Relax is a library, of libmakepad.
#[cfg(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"))]
fn data_regions() -> Vec<(String, usize, usize, usize)> {
    use crate::inspect::symbols::{dl_iterate_phdr, PhdrInfo};
    use std::ffi::CStr;
    extern "C" fn cb(info: *const PhdrInfo, _size: usize, data: *mut u8) -> i32 {
        let out = unsafe { &mut *(data as *mut Vec<(String, usize, usize, usize)>) };
        let info = unsafe { &*info };
        let name = if info.name.is_null() { String::new() } else { unsafe { CStr::from_ptr(info.name.cast()) }.to_string_lossy().into_owned() };
        let module = if name.is_empty() { "exe".to_string() } else if name.contains("makepad") || name.contains("relax") { name.rsplit('/').next().unwrap_or("lib").to_string() } else { return 0 };
        for i in 0..info.phnum as usize {
            let p = unsafe { &*(info.phdr as *const Phdr).add(i) };
            // PT_LOAD with PF_W: .data, .bss, .got and the like
            if p.kind == 1 && p.flags & 2 != 0 {
                out.push((module.clone(), info.addr as usize, info.addr as usize + p.vaddr as usize, p.memsz as usize));
            }
        }
        0
    }
    let mut out = Vec::new();
    unsafe { dl_iterate_phdr(cb, &mut out as *mut _ as *mut u8) };
    out
}

/// ELF64 program header.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
struct Phdr {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

/// macOS: the __DATA segments of the main image (makepad binary or the library that contains it).
#[cfg(target_os = "macos")]
fn data_regions() -> Vec<(String, usize, usize, usize)> {
    use std::ffi::c_char;
    extern "C" {
        fn _dyld_get_image_header(i: u32) -> *const u8;
        fn getsegmentdata(h: *const u8, name: *const c_char, size: *mut usize) -> *mut u8;
    }
    let mut out = Vec::new();
    let h = unsafe { _dyld_get_image_header(0) };
    if h.is_null() {
        return out;
    }
    for seg in [c"__DATA", c"__DATA_CONST", c"__DATA_DIRTY"] {
        let mut size = 0usize;
        let p = unsafe { getsegmentdata(h, seg.as_ptr(), &mut size) };
        if !p.is_null() && size > 0 {
            out.push(("exe".to_string(), h as usize, p as usize, size));
        }
    }
    out
}

#[cfg(not(any(all(any(target_os = "linux", target_os = "android"), target_pointer_width = "64"), target_os = "macos")))]
fn data_regions() -> Vec<(String, usize, usize, usize)> {
    Vec::new()
}

// ---- static TLS of every thread (Linux x86-64, glibc): variant II, the block of the initial
// modules lies directly below the thread pointer. The calling thread scans itself; every other
// thread of the process is sent a real-time signal whose handler (no locks, no allocation) records
// the candidate words; the exactness test runs afterwards on the calling thread.

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
fn tls_scan(g: u64) {
    use std::sync::atomic::{AtomicUsize, Ordering::*};
    const SIG: i32 = 40;
    const CAP: usize = 4096;
    static N: AtomicUsize = AtomicUsize::new(0);
    static DONE: AtomicUsize = AtomicUsize::new(0);
    static SPAN: AtomicUsize = AtomicUsize::new(0);
    static EXE_SPAN: AtomicUsize = AtomicUsize::new(0);
    static REC: [[AtomicUsize; 4]; CAP] = [const { [const { AtomicUsize::new(0) }; 4] }; CAP];
    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        // glibc: BSD semantics (the handler stays installed, interrupted calls restart)
        fn signal(sig: i32, handler: usize) -> usize;
    }
    // runs on the signalled thread (or directly on the calling one): async-signal-safe
    extern "C" fn scan_self(_: i32) {
        unsafe {
            let tid = syscall(186) as usize;
            let mut fs = 0u64;
            syscall(158, 0x1003i64, &mut fs as *mut u64);
            let fs = fs as usize;
            let mut span = SPAN.load(Relaxed);
            // the block must be mapped below the thread pointer (madvise says ENOMEM otherwise)
            for _ in 0..2 {
                let lo = (fs - span) & !4095;
                if syscall(28, lo as i64, (fs - lo) as i64, 0i64) == 0 {
                    break;
                }
                span = EXE_SPAN.load(Relaxed);
            }
            let mut a = (fs - span + 7) & !7;
            while a + 8 <= fs {
                let v = *(a as *const usize);
                if crate::zone_alloc::compiler_owns(v as *const u8) {
                    let i = N.fetch_add(1, Relaxed);
                    if i < CAP {
                        REC[i][0].store(tid, Relaxed);
                        REC[i][1].store(fs, Relaxed);
                        REC[i][2].store(a, Relaxed);
                        REC[i][3].store(v, Relaxed);
                    }
                }
                a += 8;
            }
            DONE.fetch_add(1, Release);
        }
    }
    // spans of the static TLS: all initial modules, and the executable alone
    {
        use crate::inspect::symbols::{dl_iterate_phdr, PhdrInfo};
        extern "C" fn cb(info: *const PhdrInfo, _s: usize, data: *mut u8) -> i32 {
            let tot = unsafe { &mut *(data as *mut [usize; 2]) };
            let info = unsafe { &*info };
            for i in 0..info.phnum as usize {
                let p = unsafe { &*(info.phdr as *const Phdr).add(i) };
                if p.kind == 7 {
                    let al = (p.align as usize).max(1);
                    let sz = (p.memsz as usize + al - 1) / al * al;
                    if tot[1] == 0 {
                        tot[1] = sz; // the first module is the executable
                    }
                    tot[0] += sz;
                }
            }
            0
        }
        let mut tot = [0usize; 2];
        unsafe { dl_iterate_phdr(cb, &mut tot as *mut _ as *mut u8) };
        // only the executable's block (Rust thread_locals live there): reading further below the
        // thread pointer reaches modules loaded later and the thread's stack
        SPAN.store(tot[1].min(1 << 20), Relaxed);
        EXE_SPAN.store(tot[1].min(1 << 20), Relaxed);
    }
    N.store(0, Relaxed);
    DONE.store(0, Release);
    let me = unsafe { syscall(186) } as usize;
    let pid = std::process::id() as i64;
    let old = unsafe { signal(SIG, scan_self as *const () as usize) };
    let mut sent = 0usize;
    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
        for e in dir.flatten() {
            let Some(tid) = e.file_name().to_str().and_then(|n| n.parse::<usize>().ok()) else { continue };
            if tid != me && unsafe { syscall(234, pid, tid as i64, SIG as i64) } == 0 {
                sent += 1;
            }
        }
    }
    scan_self(0);
    let t0 = std::time::Instant::now();
    while DONE.load(Acquire) < sent + 1 && t0.elapsed() < std::time::Duration::from_millis(500) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let done = DONE.load(Acquire);
    unsafe { signal(SIG, old) };
    // exactness, on this thread: the TLS image of the executable starts at tp - exe span
    let exe_span = EXE_SPAN.load(Relaxed);
    let mut hits = Vec::new();
    for r in REC.iter().take(N.load(Relaxed).min(CAP)) {
        let (tid, fs, a, v) = (r[0].load(Relaxed), r[1].load(Relaxed), r[2].load(Relaxed), r[3].load(Relaxed));
        if crate::zone_alloc::compiler_points_into(v, g) {
            let off = a as isize - (fs - exe_span) as isize;
            let place = format!("tid {tid} tls[exe]+{off:#x}");
            hits.push(Hit { place, target: v });
        }
    }
    // the record table is a static: leave no arena address in it for the next static scan
    for r in REC.iter() {
        r[3].store(0, Relaxed);
    }
    print_hits("tls scan", &hits, &format!(" ({done} of {} threads answered)", sent + 1));
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")))]
fn tls_scan(_g: u64) {
    eprintln!("makepad-run zone: tls scan: not available on this target (statics only)");
}

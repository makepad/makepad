//! `memory`: where the process's memory is and why, now and over time (DESIGN.md §5).
//!
//! One snapshot is a flat list of named byte counts (nodes, `/`-separated paths):
//! - `rss/...` (Linux): the resident set split by what smaps says each mapping is (the heap,
//!   relax's named mappings, GPU driver mappings, the relax cache files, libraries, ...); these
//!   add up to the RSS. `regions/...` (macOS): resident pages by VM tag, private vs shared --
//!   shared file pages are counted whole, so they do not add up to the kernel's resident size.
//! - `malloc/...`: the allocator's own view: in use, held free (the retention a trim returns).
//! - `relax/...`: what Relax holds for the app: JIT code / data / statics, the burst arena, the
//!   compiler's heap by subsystem when MAKEPAD_RUN_MEM_REPORT counts it, structure sizes on request.
//!
//! No owner needed (structure sizes excepted, asked with a timeout); nothing kept running but a
//! resident-size ring the detector fills four times a second, with phase markers.

use super::{json_str, now_ms, Out, Req};
use relax_session::sync::Mutex;

pub struct Snap {
    pub at_ms: u64,
    pub nodes: Vec<(String, u64)>,
}

impl Snap {
    fn get(&self, k: &str) -> u64 {
        self.nodes.iter().find(|n| n.0 == k).map_or(0, |n| n.1)
    }
    fn add(&mut self, k: &str, v: u64) {
        match self.nodes.iter_mut().find(|n| n.0 == k) {
            Some(n) => n.1 += v,
            None => self.nodes.push((k.to_string(), v)),
        }
    }
}

const MB: f64 = 1048576.0;

fn mb(v: u64) -> String {
    format!("{:.1}", v as f64 / MB)
}

fn smb(v: i64) -> String {
    format!("{:+.1}", v as f64 / MB)
}

// ------------------------------------------------------------------ the timeline ring

/// (ms, resident bytes); every other point is dropped when full (the start is kept, resolution
/// halves)
static RING: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
static RING_STEP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static MARKS: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
const RING_MAX: usize = 4096;

/// A detector tick: the resident size now (every `RING_STEP`-th tick once the ring halved).
pub fn tick(n: u64, rss: u64) {
    let step = RING_STEP.load(std::sync::atomic::Ordering::Relaxed);
    if n % step != 0 {
        return;
    }
    let _k = crate::zone_alloc::kept();
    let mut r = RING.lock().unwrap_or_else(|e| e.into_inner());
    r.push((now_ms(), rss));
    if r.len() >= RING_MAX {
        let kept: Vec<(u64, u64)> = r.iter().copied().step_by(2).collect();
        *r = kept;
        RING_STEP.store(step * 2, std::sync::atomic::Ordering::Relaxed);
    }
}

/// A phase marker on the memory timeline (main start, first present, snapshots, the app's marks).
pub fn mark(label: &str) {
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() < 1024 {
        crate::zone_alloc::kept_with(|| m.push((now_ms(), label.to_string())));
    }
}

// ------------------------------------------------------------------ taking a snapshot

pub fn take(compiler: bool, partial: &mut Vec<String>) -> Snap {
    let mut s = Snap { at_ms: now_ms(), nodes: Vec::new() };
    os_regions(&mut s);
    malloc_stats(&mut s);
    relax_parts(&mut s, compiler, partial);
    s
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn os_regions(s: &mut Snap) {
    let text = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let home = std::env::var("MAKEPAD_RUN_HOME").unwrap_or_default();
    let mut cat = String::new();
    let (mut rss, mut anon) = (0u64, 0u64);
    let flush = |s: &mut Snap, cat: &str, rss: u64, anon: u64| {
        if rss == 0 {
            return;
        }
        s.add("rss", rss);
        s.add(&format!("rss/anon/{}", cat), anon);
        if rss > anon {
            s.add(&format!("rss/file/{}", cat), rss - anon);
        }
    };
    for line in text.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.contains('-') && first.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            flush(s, &cat, rss, anon);
            rss = 0;
            anon = 0;
            let path = line.split_whitespace().nth(5).unwrap_or("");
            cat = classify_path(path, &home);
        } else if let Some(v) = line.strip_prefix("Rss:") {
            rss = kb(v);
        } else if let Some(v) = line.strip_prefix("Anonymous:") {
            anon = kb(v);
        }
    }
    flush(s, &cat, rss, anon);
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for l in status.lines() {
        if let Some(v) = l.strip_prefix("VmHWM:") {
            s.add("peak_rss", kb(v));
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn kb(v: &str) -> u64 {
    v.split_whitespace().next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) * 1024
}

/// What a mapping is, from its smaps name.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn classify_path(path: &str, home: &str) -> String {
    match path {
        "" => "unnamed (malloc arenas + large blocks, thread stacks, driver memory)".into(),
        "[heap]" => "malloc main heap".into(),
        "[stack]" => "main stack".into(),
        "[vdso]" | "[vvar]" | "[vsyscall]" | "[vvar_vclock]" => "kernel".into(),
        p if p.starts_with("[anon:makepad-run-jit-code") => "makepad run JIT code".into(),
        p if p.starts_with("[anon:makepad-run-jit-data") => "makepad run JIT data".into(),
        p if p.starts_with("[anon:makepad-run-burst") => "makepad run burst arena".into(),
        p if p.starts_with("[anon:makepad-run-stack") => "makepad run compile stacks".into(),
        p if p.starts_with("[anon:") => format!("named {}", p.trim_start_matches("[anon:").trim_end_matches(']')),
        p if p.starts_with("/dev/dri") || p.starts_with("/dev/nvidia") || p.contains("/drm") => "GPU driver mappings".into(),
        p if p.starts_with("/memfd:") => format!("memfd {}", p.trim_start_matches("/memfd:").split(' ').next().unwrap_or("")),
        p if p.contains("/.makepad-run/") || p.contains("/makepad-run/") || (!home.is_empty() && p.starts_with(home)) => "makepad run cache files".into(),
        p if p.contains(".so") => "shared libraries".into(),
        p if p.ends_with("/makepad") || p.contains("target/release/makepad") => "makepad binary".into(),
        p if p.ends_with(".ttf") || p.ends_with(".otf") || p.ends_with(".ttc") => "fonts".into(),
        _ => "other files".into(),
    }
}

#[cfg(target_os = "macos")]
fn os_regions(s: &mut Snap) {
    extern "C" {
        static mach_task_self_: u32;
        static vm_page_size: usize;
        fn task_info(task: u32, flavor: i32, info: *mut u64, count: *mut u32) -> i32;
        fn mach_vm_region_recurse(task: u32, addr: *mut u64, size: *mut u64, depth: *mut u32, info: *mut u32, count: *mut u32) -> i32;
    }
    let task = unsafe { mach_task_self_ };
    // TASK_VM_INFO: resident 2, resident peak 3, internal 6, external 8, compressed 15, phys_footprint 18
    let mut vm = [0u64; 24];
    let mut count: u32 = 38;
    if unsafe { task_info(task, 22, vm.as_mut_ptr(), &mut count) } == 0 {
        // the resident size is the kernel's (the region walk below counts pages a file and other
        // processes share too, so its parts do not add up to it)
        s.add("rss", vm[2]);
        s.add("footprint", vm[18]);
        s.add("peak_rss", vm[3]);
        s.add("compressed", vm[15]);
    }
    let page = unsafe { vm_page_size } as u64;
    let (mut addr, mut depth) = (0u64, 0u32);
    // a VM object mapped more than once (Relax's JIT code has a writable and an executable view;
    // shared libraries' pages) counts once
    let mut seen_objects: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for _ in 0..200_000 {
        let mut size = 0u64;
        let mut info = [0u32; 19];
        let mut n = 19u32;
        if unsafe { mach_vm_region_recurse(task, &mut addr, &mut size, &mut depth, info.as_mut_ptr(), &mut n) } != 0 {
            break;
        }
        // vm_region_submap_info_64 (pack 4): user_tag 5, pages_resident 6, share_mode byte 3 of 11, is_submap 12
        if info[12] != 0 {
            depth += 1;
            continue;
        }
        let resident = info[6] as u64 * page;
        let object = (info[17] as u64) | ((info[18] as u64) << 32);
        if resident > 0 && (object == 0 || seen_objects.insert(object)) {
            let share_mode = (info[11] >> 24) & 0xff;
            // SM_COW 1, SM_PRIVATE 2, SM_EMPTY 3, SM_SHARED 4, SM_TRUESHARED 5, SM_PRIVATE_ALIASED 6,
            // SM_SHARED_ALIASED 7
            let shared = matches!(share_mode, 4 | 5 | 7);
            let cat = tag_name(info[5]);
            s.add(&format!("regions/{}/{}", if shared { "shared" } else { "private" }, cat), resident);
        }
        addr += size;
    }
}

/// A macOS VM region tag (mach/vm_statistics.h VM_MEMORY_*) as a category.
#[cfg(target_os = "macos")]
fn tag_name(t: u32) -> &'static str {
    match t {
        0 => "untagged (makepad run JIT code/data, burst arena, mapped files)",
        1 | 2 | 3 | 4 | 6 | 7 | 8 | 9 | 11 | 12 => "malloc",
        5 => "sbrk",
        20 => "mach msg",
        21 => "IOKit",
        30 => "stacks",
        31 => "guard",
        32 => "shared pmap",
        33 | 60..=62 => "dylibs / dyld",
        34 => "Objective-C",
        35..=39 => "Foundation / CoreServices",
        40..=49 => "CoreGraphics / CoreImage",
        50..=59 => "CoreAudio / CoreMedia / JS",
        63..=69 => "image IO / layer kit",
        80 => "GPU (IOAccelerator)",
        81 => "IOSurface",
        _ => "other tagged",
    }
}

#[cfg(windows)]
fn os_regions(s: &mut Snap) {
    s.add("rss", crate::memstat::resident());
}

#[cfg(target_os = "linux")]
fn malloc_stats(s: &mut Snap) {
    #[repr(C)]
    struct Mallinfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }
    extern "C" {
        fn mallinfo2() -> Mallinfo2;
    }
    let m = unsafe { mallinfo2() };
    s.add("malloc/in use", (m.uordblks + m.hblkhd) as u64);
    s.add("malloc/in use/large blocks (mmapped)", m.hblkhd as u64);
    s.add("malloc/held free", m.fordblks as u64);
}

#[cfg(target_os = "macos")]
fn malloc_stats(s: &mut Snap) {
    #[repr(C)]
    #[derive(Default)]
    struct Stats {
        blocks_in_use: u32,
        size_in_use: usize,
        max_size_in_use: usize,
        size_allocated: usize,
    }
    extern "C" {
        static mach_task_self_: u32;
        fn malloc_get_all_zones(task: u32, reader: *const u8, out: *mut *const usize, count: *mut u32) -> i32;
        fn malloc_zone_statistics(zone: *const u8, stats: *mut Stats);
        fn malloc_get_zone_name(zone: *const u8) -> *const i8;
    }
    let mut zones: *const usize = std::ptr::null();
    let mut n = 0u32;
    if unsafe { malloc_get_all_zones(mach_task_self_, std::ptr::null(), &mut zones, &mut n) } != 0 {
        return;
    }
    for i in 0..n as usize {
        let z = unsafe { *zones.add(i) } as *const u8;
        if z.is_null() {
            continue;
        }
        let mut st = Stats::default();
        unsafe { malloc_zone_statistics(z, &mut st) };
        let name = unsafe {
            let p = malloc_get_zone_name(z);
            if p.is_null() { "zone".to_string() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        s.add("malloc/in use", st.size_in_use as u64);
        s.add(&format!("malloc/in use/{}", name), st.size_in_use as u64);
        s.add("malloc/held free", st.size_allocated.saturating_sub(st.size_in_use) as u64);
    }
}

#[cfg(any(windows, target_os = "android"))]
/// bionic has no mallinfo2 before API 34.
fn malloc_stats(_s: &mut Snap) {}

fn relax_parts(s: &mut Snap, compiler: bool, partial: &mut Vec<String>) {
    let up = crate::jit::current_unit();
    if !up.is_null() {
        let (code, data, rw) = unsafe { &*up }.reserved_use();
        s.add("makepad-run/JIT code (incl. thunks, stubs)", code);
        s.add("makepad-run/JIT read-only data", data);
        s.add("makepad-run/program statics", rw);
    }
    let (program, host) = super::heap::live_totals();
    if let Some(live) = program {
        s.add("malloc/in use/program (tracked stacks, est.)", live.max(0) as u64);
    }
    if let Some(live) = host {
        s.add("malloc/in use/makepad run + compiled-in crates (tracked stacks, est.)", live.max(0) as u64);
    }
    // Splash VMs (their registry's estimates, as of their last evaluation / collection)
    for v in super::vm_summaries() {
        if let Ok(j) = crate::json::parse(v.as_bytes()) {
            let b = j.get("heap_bytes_est").map_or(0.0, |x| x.num()) as u64;
            if b > 0 {
                let id = j.get("vm").map(|x| x.str().to_string()).unwrap_or_default();
                let label = j.get("label").map(|x| x.str().to_string()).unwrap_or_default();
                s.add(&format!("splash/{} {}", label, id), b);
            }
        }
    }
    let (committed, live, _) = crate::zone_alloc::burst_stats();
    s.add("makepad-run/burst arena committed", committed as u64);
    s.add("makepad-run/burst arena live", live as u64);
    let (ccommitted, clive, cchunks) = crate::zone_alloc::compiler_stats();
    s.add("makepad-run/compiler arena committed", ccommitted as u64);
    s.add("makepad-run/compiler arena live", clive as u64);
    s.add("makepad-run/compiler arena/chunks (2 MB)", cchunks as u64);
    let (touched, slots, large, listed) = crate::zone_alloc::compiler_detail();
    s.add("makepad-run/compiler arena/small chunks touched", touched as u64);
    s.add("makepad-run/compiler arena/small live slots", slots as u64);
    s.add("makepad-run/compiler arena/large blocks", large as u64);
    s.add("makepad-run/compiler arena/chunks listed for reuse", listed as u64);
    if crate::memstat::enabled() {
        for (i, name) in crate::memstat::TAG_NAMES.iter().enumerate() {
            let v = crate::memstat::live_of_index(i);
            if v > 0 {
                s.add(&format!("makepad-run/compiler heap (MAKEPAD_RUN_MEM_REPORT)/{}", name), v as u64);
            }
        }
    }
    if compiler && !up.is_null() {
        let r = crate::owner::call_within(crate::owner::Class::Background, 3000, move || crate::memreport::unit_sizes(unsafe { &*up }));
        match r {
            Some(rows) => {
                for (k, v) in rows {
                    if v > 0 && !k.starts_with(' ') {
                        s.add(&format!("makepad-run/compiler structures/{}", k), v);
                    }
                }
            }
            None => partial.push(format!("compiler structures: owner busy ({})", crate::owner::serving_note())),
        }
    }
}

// ------------------------------------------------------------------ commands

static SNAPS: Mutex<Vec<(String, Snap)>> = Mutex::new(Vec::new());
const SNAPS_MAX: usize = 32;

fn keep(name: &str, s: Snap) {
    let _k = crate::zone_alloc::kept();
    let mut v = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|x| x.0 != name);
    if v.len() >= SNAPS_MAX {
        v.remove(0);
    }
    // The snapshot may have been measured in the compiler phase; copy its owned nodes.
    v.push((name.to_string(), Snap { at_ms: s.at_ms, nodes: s.nodes.clone() }));
}

/// `memory [compiler=1] [n=24]`: the snapshot as a ranked tree plus findings, and the growth since
/// the previous `memory` (kept as snapshot `last`).
pub fn cmd_memory(req: &Req, out: &mut Out) {
    out.line(&format!("{{\"freed_by_domain_teardown_bytes\":{}}}", crate::domain::freed_by_teardown()));
    for d in crate::domain::list() {
        out.line(&format!("{{\"domain\":{},\"package\":{},\"state\":{},\"arena_in_use_bytes\":{},\"arena_committed_bytes\":{},\"static_committed_bytes\":{},\"threads\":{},\"thread_names\":[{}]}}",
            d.id, json_str(&d.package), json_str(&format!("{:?}", d.state)), d.arena_in_use, d.arena_committed, d.static_committed, d.threads,
            d.thread_names.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",")));
    }
    let compiler = req.get("compiler") == Some("1");
    let n = req.num("n", 24) as usize;
    let mut partial = Vec::new();
    let snap = take(compiler, &mut partial);
    let prev = SNAPS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|x| x.0 == "last").map(|x| x.1.nodes.clone());
    out.measured("memory now: OS mappings, allocator, makepad run's own parts", "os + malloc + makepad run counters (no owner unless compiler=1)", &partial);
    let rss = snap.get("rss");
    let mut total = format!("{{\"rss_mb\":{},\"peak_rss_mb\":{}", mb(rss), mb(snap.get("peak_rss")));
    if snap.get("footprint") > 0 {
        total.push_str(&format!(",\"footprint_mb\":{},\"compressed_mb\":{}", mb(snap.get("footprint")), mb(snap.get("compressed"))));
    }
    let (inuse, free) = (snap.get("malloc/in use"), snap.get("malloc/held free"));
    total.push_str(&format!(",\"malloc_in_use_mb\":{},\"malloc_held_free_mb\":{}", mb(inuse), mb(free)));
    if let Some(p) = &prev {
        let was = p.iter().find(|x| x.0 == "rss").map_or(0, |x| x.1);
        total.push_str(&format!(",\"rss_since_last_mb\":{}", smb(rss as i64 - was as i64)));
    }
    total.push('}');
    out.line(&total);
    let ev = crate::zone_alloc::compiler_events();
    // the three heaps apart (zone_alloc.rs): the program's malloc, the compiler's own arena, the JIT's mappings
    out.line(&format!(
        "{{\"split\":{{\"routing\":{},\"program_heap_in_use_mb\":{},\"program_heap_held_free_mb\":{},\"compiler_arena_committed_mb\":{},\"compiler_arena_live_mb\":{},\"jit_code_mb\":{},\"jit_ro_data_mb\":{},\"program_statics_mb\":{},\"burst_arena_committed_mb\":{},\"compiler_arena_touched_mb\":{},\"compiler_arena_live_slots_mb\":{},\"compiler_arena_large_mb\":{},\"compiler_arena_chunks_listed\":{},\"compiler_arena_events\":{{\"contended_chunk\":{},\"contended_pool\":{},\"contended_warm\":{},\"contended_partial\":{},\"yields\":{},\"commits\":{},\"decommits\":{},\"pushes\":{},\"pops\":{},\"tombstone_frees\":{},\"generation\":{},\"tombstones\":{}}}{}}}}}",
        json_str(if crate::zone_alloc::compiler_arena_enabled() { "compiler arena" } else { "off (MAKEPAD_RUN_COMPILER_ARENA=0)" }),
        mb(inuse),
        mb(free),
        mb(snap.get("makepad-run/compiler arena committed")),
        mb(snap.get("makepad-run/compiler arena live")),
        mb(snap.get("makepad-run/JIT code (incl. thunks, stubs)")),
        mb(snap.get("makepad-run/JIT read-only data")),
        mb(snap.get("makepad-run/program statics")),
        mb(snap.get("makepad-run/burst arena committed")),
        mb(snap.get("makepad-run/compiler arena/small chunks touched")),
        mb(snap.get("makepad-run/compiler arena/small live slots")),
        mb(snap.get("makepad-run/compiler arena/large blocks")),
        snap.get("makepad-run/compiler arena/chunks listed for reuse"),
        ev[0], ev[1], ev[2], ev[3], ev[4], ev[5], ev[6], ev[7], ev[8],
        crate::zone_alloc::compiler_tombstone_frees(), crate::zone_alloc::compiler_generation(), crate::zone_alloc::compiler_tombstones(),
        match crate::jit::zone_scan::last_total() { Some(n) => format!(",\"kept_in_compiler\":{n}"), None => String::new() },
    ));
    // the tree: every node, ranked within its parent, top `n`
    let mut rows: Vec<&(String, u64)> = snap.nodes.iter().filter(|x| x.0.contains('/') && x.1 >= 1 << 20).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (k, v) in rows.iter().take(n) {
        let base = if k.starts_with("malloc") { inuse.max(1) } else { rss };
        let delta = prev.as_ref().map(|p| p.iter().find(|x| &x.0 == k).map_or(*v as i64, |x| *v as i64 - x.1 as i64));
        let d = delta.filter(|d| d.unsigned_abs() >= 1 << 20).map(|d| format!(",\"since_last_mb\":{}", smb(d))).unwrap_or_default();
        out.line(&format!("{{\"node\":{},\"mb\":{},\"pct\":{:.0}{}}}", json_str(k), mb(*v), *v as f64 * 100.0 / base.max(1) as f64, d));
    }
    // findings, ranked by bytes
    let mut f: Vec<(u64, String)> = Vec::new();
    if free > 64 << 20 {
        f.push((free, format!("malloc holds {} MB freed but not returned (retention; {:.0}% of its in-use bytes): a trim returns it", mb(free), free as f64 * 100.0 / inuse.max(1) as f64)));
    }
    let relax: u64 = snap.nodes.iter().filter(|x| x.0.starts_with("makepad-run/") && !x.0["makepad-run/".len()..].contains('/') && !x.0.ends_with(" live")).map(|x| x.1).sum();
    if relax > 0 {
        f.push((relax, format!("makepad run's own reservations in use: {} MB (JIT code + data + statics + burst and compiler arenas)", mb(relax))));
    }
    if let Some(p) = &prev {
        let mut growth: Vec<(i64, &String)> = snap.nodes.iter().map(|(k, v)| (*v as i64 - p.iter().find(|x| &x.0 == k).map_or(0, |x| x.1 as i64), k)).filter(|x| x.0 >= 16 << 20 && x.1.contains('/')).collect();
        growth.sort_by(|a, b| b.0.cmp(&a.0));
        for (d, k) in growth.iter().take(3) {
            f.push((*d as u64, format!("{} grew {} MB since the last `memory`", k, smb(*d))));
        }
    }
    let tracked = snap.get("malloc/in use/program (tracked stacks, est.)");
    if tracked > 0 {
        f.push((tracked, format!("program heap (tracked since `heap start`): {} MB live: `heap top` for the stacks", mb(tracked))));
    } else if !crate::memstat::enabled() {
        f.push((0, "program heap vs makepad run's heap is not split yet: `heap start` tracks the program's allocations by stack, `heap start host=1` makepad run's own (compiler + compiled-in crates, by stack and category)".into()));
    }
    f.sort_by(|a, b| b.0.cmp(&a.0));
    let lines: Vec<String> = f.iter().map(|x| json_str(&x.1)).collect();
    out.line(&format!("{{\"findings\":[{}],\"more\":[\"memory compiler=1\",\"mem timeline\",\"mem snap <name>\",\"diff <a> <b|now>\"]}}", lines.join(",")));
    keep("last", snap);
}

/// `mem snap <name>` / `mem timeline [step_ms=]` / `mem marks`.
pub fn cmd_mem(req: &Req, out: &mut Out) {
    match req.pos.first().map(String::as_str) {
        Some("snap") => {
            let name = req.pos.get(1).cloned().unwrap_or_else(|| format!("s{}", now_ms()));
            let mut partial = Vec::new();
            let s = take(req.get("compiler") == Some("1"), &mut partial);
            out.measured(&format!("snapshot `{}`", name), "os + malloc + makepad run counters", &partial);
            out.line(&format!("{{\"snap\":{},\"rss_mb\":{},\"nodes\":{}}}", json_str(&name), mb(s.get("rss")), s.nodes.len()));
            mark(&format!("snap {}", name));
            keep(&name, s);
        }
        Some("timeline") => {
            let r = RING.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let marks = MARKS.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let from = req.num("from_ms", 0);
            let to = req.num("to_ms", u64::MAX);
            let pts: Vec<&(u64, u64)> = r.iter().filter(|p| p.0 >= from && p.0 <= to).collect();
            out.measured("resident size over time (detector ring) with phase markers", "ring", &[]);
            // at most 60 points: evenly picked, the peak always in
            let peak = pts.iter().max_by_key(|p| p.1).copied();
            let stride = (pts.len() / 60).max(1);
            let mut shown: Vec<(u64, u64)> = pts.iter().step_by(stride).map(|p| **p).collect();
            if let Some(p) = peak {
                if !shown.contains(p) {
                    shown.push(*p);
                    shown.sort();
                }
            }
            let s: Vec<String> = shown.iter().map(|(t, v)| format!("[{},{}]", t, v >> 20)).collect();
            out.line(&format!("{{\"rss_mb_at_ms\":[{}]}}", s.join(",")));
            if let Some(p) = peak {
                let phase = marks.iter().rev().find(|m| m.0 <= p.0).map(|m| m.1.clone()).unwrap_or_else(|| "start".into());
                out.line(&format!("{{\"peak\":{{\"rss_mb\":{},\"at_ms\":{},\"after\":{}}}}}", p.1 >> 20, p.0, json_str(&phase)));
            }
            let m: Vec<String> = marks.iter().filter(|m| m.0 >= from && m.0 <= to).map(|(t, l)| format!("[{},{}]", t, json_str(l))).collect();
            out.line(&format!("{{\"marks\":[{}]}}", m.join(",")));
        }
        _ => out.err("usage: mem snap <name> [compiler=1] | mem timeline [from_ms=] [to_ms=]"),
    }
}

/// `diff <a> <b|now>`: growth per node between two snapshots, largest first.
pub fn cmd_diff(req: &Req, out: &mut Out) {
    let (Some(a), Some(b)) = (req.pos.first(), req.pos.get(1).or(Some(&"now".to_string())).cloned()) else {
        out.err("usage: diff <snap a> <snap b|now>");
        return;
    };
    let mut partial = Vec::new();
    let snaps = SNAPS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sa) = snaps.iter().find(|x| &x.0 == a).map(|x| (x.1.at_ms, x.1.nodes.clone())) else {
        let names: Vec<&str> = snaps.iter().map(|x| x.0.as_str()).collect();
        out.err(&format!("no snapshot `{}` (have: {})", a, names.join(", ")));
        return;
    };
    let sb = if b == "now" {
        drop(snaps);
        let s = take(false, &mut partial);
        (s.at_ms, s.nodes)
    } else {
        match snaps.iter().find(|x| x.0 == b).map(|x| (x.1.at_ms, x.1.nodes.clone())) {
            Some(s) => s,
            None => {
                out.err(&format!("no snapshot `{}`", b));
                return;
            }
        }
    };
    out.measured(&format!("memory growth from `{}` ({} ms) to `{}` ({} ms)", a, sa.0, b, sb.0), "snapshots", &partial);
    let mut keys: Vec<&String> = sa.1.iter().map(|x| &x.0).chain(sb.1.iter().map(|x| &x.0)).collect();
    keys.sort();
    keys.dedup();
    let val = |v: &Vec<(String, u64)>, k: &str| v.iter().find(|x| x.0 == k).map_or(0, |x| x.1) as i64;
    let mut d: Vec<(i64, &String)> = keys.iter().map(|k| (val(&sb.1, k) - val(&sa.1, k), *k)).filter(|x| x.0.unsigned_abs() >= 1 << 20).collect();
    d.sort_by(|x, y| y.0.abs().cmp(&x.0.abs()).then(x.1.cmp(y.1)));
    for (delta, k) in d.iter().take(req.num("n", 20) as usize) {
        out.line(&format!("{{\"node\":{},\"delta_mb\":{},\"from_mb\":{},\"to_mb\":{}}}", json_str(k), smb(*delta), mb(val(&sa.1, k) as u64), mb(val(&sb.1, k) as u64)));
    }
}

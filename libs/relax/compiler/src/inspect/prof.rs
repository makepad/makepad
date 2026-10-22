//! The sampling profiler (DESIGN.md §7): where CPU time goes, per thread, JIT and host frames
//! together, without the owner and without a debugger.
//!
//! - Linux: per-thread perf_event task-clock samples carrying the user registers and a copy of the
//!   top of the stack (PERF_SAMPLE_REGS_USER | PERF_SAMPLE_STACK_USER, as `perf --call-graph
//!   dwarf` does); the walk (CFI for host frames, frame records for JIT frames) runs here on the
//!   copy. No signal reaches the program.
//! - macOS: the threads whose CPU time moved are suspended one at a time and walked (os capture),
//!   at most `hz` times a second.
//!
//! Samples are CPU time only: waiting threads do not show (why-hung and the wait registry say what
//! they wait for).

use super::{frame_json, json_str, now_ms, short, Out, Req};

pub struct Sample {
    pub os_id: u64,
    pub frames: Vec<u64>,
    /// events this sample stands for (hardware events: the period the kernel sampled at; CPU
    /// time: 1)
    pub weight: u64,
    /// when it was taken (clock::wall_ns; Linux: when its ring was drained, within 10 ms)
    pub t: u64,
}

/// What a sample counts. CPU time works everywhere; the hardware events need Linux perf_event
/// (macOS gives an unprivileged process no per-thread hardware sampling).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ev {
    Cpu,
    Cycles,
    Instructions,
    BranchMisses,
    CacheMisses,
}

impl Ev {
    pub fn parse(s: &str) -> Result<Ev, String> {
        Ok(match s {
            "cpu" | "time" | "task-clock" => Ev::Cpu,
            "cycles" => Ev::Cycles,
            "instructions" | "ins" => Ev::Instructions,
            "branch-misses" | "branches" => Ev::BranchMisses,
            "cache-misses" | "cache" => Ev::CacheMisses,
            _ => return Err(format!("ev={}: cpu | cycles | instructions | branch-misses | cache-misses", s)),
        })
    }
    /// The sampling period of a hardware event for about `hz` samples per second of a busy
    /// thread (~4 G cycles or instructions a second; misses far fewer). Linux only (perf_event).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn period(self, hz: u64) -> u64 {
        let per_s: u64 = match self {
            Ev::Cpu => 1_000_000_000,
            Ev::Cycles | Ev::Instructions => 4_000_000_000,
            Ev::BranchMisses => 40_000_000,
            Ev::CacheMisses => 4_000_000,
        };
        (per_s / hz.max(1)).max(1)
    }

    pub fn name(self) -> &'static str {
        match self {
            Ev::Cpu => "CPU time",
            Ev::Cycles => "cycles",
            Ev::Instructions => "instructions",
            Ev::BranchMisses => "branch misses",
            Ev::CacheMisses => "cache misses",
        }
    }
}

pub struct Run {
    pub samples: Vec<Sample>,
    pub threads: Vec<super::threads::Thread>,
    pub secs: f64,
    pub hz: u64,
    pub lost: u64,
    /// why sampling stopped early or skipped rounds (client gone, capture lock busy)
    pub notes: Vec<String>,
    /// what the samples count (a hardware event falls back to CPU time where it cannot be opened)
    pub ev: Ev,
}

/// Collects samples for `secs` at `hz` per thread of CPU time, from the threads `want` picks
/// (never inspect's own: the caller would suspend itself). Stops early once `cancel` says the
/// client is gone.
pub fn collect(secs: f64, hz: u64, want: &dyn Fn(&super::threads::Thread) -> bool, cancel: &dyn Fn() -> bool) -> Run {
    collect_ev(secs, hz, Ev::Cpu, want, cancel)
}

/// `collect` counting `ev`: hardware events are sampled about `hz` times a second per thread
/// (the kernel adapts the period), each sample weighted by its period.
pub fn collect_ev(secs: f64, hz: u64, ev: Ev, want: &dyn Fn(&super::threads::Thread) -> bool, cancel: &dyn Fn() -> bool) -> Run {
    super::cfi::refresh();
    let skip = super::SERVER_TIDS.lock().unwrap().clone();
    let mut threads = super::threads::list();
    super::name_handles(&mut threads);
    threads.retain(|t| !super::is_inspect_thread(t, &skip) && want(t));
    let mut run = imp::collect(threads, secs, hz, ev, cancel);
    if cancel() {
        run.notes.push(format!("stopped after {:.1} s: the client went away", run.secs));
    }
    run
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Run, Sample};
    use crate::inspect::cfi::Regs;

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn munmap(a: *mut u8, len: usize) -> i32;
        fn close(fd: i32) -> i32;
    }
    const PAGE: usize = 4096;
    /// data pages of a ring (a power of two): at most, and the least a ring is opened with (one
    /// sample with its stack copy is ~16.5 KB)
    const RING_PAGES: usize = 256;
    const MIN_PAGES: usize = 16;
    const STACK_COPY: u32 = 16384;
    /// perf regs (x86): bx 1, bp 6, sp 7, ip 8, r12..r15 20..23 -- ascending order in a sample
    const REGS_MASK: u64 = (1 << 1) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);

    struct Ring {
        fd: i32,
        base: usize,
        os_id: u64,
        pages: usize,
    }

    /// Why a ring did not open: the perf_event_open errno, or the mmap's (the locked-memory
    /// budget: RLIMIT_MEMLOCK plus the user's perf_event_mlock_kb x CPUs, shared with every perf
    /// user of this uid -- 165 ran out at ~8-24 rings of 1 MB with ~100 program threads, and the
    /// busy UI thread got none: no JIT samples).
    enum OpenErr {
        Event(i64),
        Map,
    }

    fn open(tid: u64, hz: u64, ev: super::Ev, pages: usize) -> Result<Ring, OpenErr> {
        let mut attr = [0u8; 128];
        let put = |a: &mut [u8; 128], off: usize, v: &[u8]| a[off..off + v.len()].copy_from_slice(v);
        // exclude_kernel | exclude_hv
        let flags = (1u64 << 5) | (1u64 << 6);
        match ev {
            super::Ev::Cpu => {
                put(&mut attr, 0, &1u32.to_ne_bytes()); // PERF_TYPE_SOFTWARE
                put(&mut attr, 8, &1u64.to_ne_bytes()); // PERF_COUNT_SW_TASK_CLOCK
                put(&mut attr, 16, &(1_000_000_000 / hz.max(1)).to_ne_bytes()); // period, ns of CPU time
            }
            hw => {
                put(&mut attr, 0, &0u32.to_ne_bytes()); // PERF_TYPE_HARDWARE
                let config: u64 = match hw {
                    super::Ev::Cycles => 0,
                    super::Ev::Instructions => 1,
                    super::Ev::CacheMisses => 3,
                    _ => 5, // branch misses
                };
                put(&mut attr, 8, &config.to_ne_bytes());
                // a fixed period (each sample stands for exactly that many events: weights stay
                // exact when the kernel throttles the stack-copying samples, which freq mode's
                // adapted periods do not), about `hz` samples a second of a busy thread
                put(&mut attr, 16, &super::Ev::period(hw, hz).to_ne_bytes());
            }
        }
        put(&mut attr, 4, &128u32.to_ne_bytes());
        put(&mut attr, 24, &(1u64 | 2 | 0x100 | 0x1000 | 0x2000).to_ne_bytes()); // IP | TID | PERIOD | REGS_USER | STACK_USER
        put(&mut attr, 40, &flags.to_ne_bytes());
        put(&mut attr, 80, &REGS_MASK.to_ne_bytes());
        put(&mut attr, 88, &STACK_COPY.to_ne_bytes());
        let fd = unsafe { syscall(298, attr.as_ptr(), tid as i32, -1i32, -1i32, 0u64) } as i32;
        if fd < 0 {
            return Err(OpenErr::Event(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as i64));
        }
        // the largest ring the locked-memory budget still gives (halved until it maps)
        let mut pages = pages;
        loop {
            let len = (1 + pages) * PAGE;
            let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 1, fd, 0) };
            if p as isize != -1 {
                return Ok(Ring { fd, base: p as usize, os_id: tid, pages });
            }
            if pages <= MIN_PAGES {
                unsafe { close(fd) };
                return Err(OpenErr::Map);
            }
            pages /= 2;
        }
    }

    /// The threads worth a ring, busiest first: those whose CPU time moved in a 30-ms look (a
    /// thread idle then is very likely idle in the run), then the rest by CPU so far. Every thread
    /// gets one while the budget lasts; the busy ones are opened first, so a short budget is spent
    /// on them.
    fn by_activity(threads: &[crate::inspect::threads::Thread]) -> Vec<usize> {
        let before: Vec<(u64, u64)> = threads.iter().map(|t| (t.os_id, t.cpu_ns)).collect();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let now = crate::inspect::threads::list();
        let mut order: Vec<(u64, u64, usize)> = before
            .iter()
            .enumerate()
            .map(|(k, &(id, c0))| {
                let c1 = now.iter().find(|t| t.os_id == id).map_or(c0, |t| t.cpu_ns);
                (c1.saturating_sub(c0), c1, k)
            })
            .collect();
        order.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        order.into_iter().map(|x| x.2).collect()
    }

    fn drain(r: &Ring, out: &mut Vec<Sample>, lost: &mut u64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        let data = r.base + PAGE;
        let size = (r.pages * PAGE) as u64;
        let head = unsafe { (*((r.base + 1024) as *const AtomicU64)).load(Ordering::Acquire) };
        let tail_p = (r.base + 1032) as *const AtomicU64;
        let mut tail = unsafe { (*tail_p).load(Ordering::Relaxed) };
        // records can wrap: copy each out first
        let copy = |off: u64, len: usize| -> Vec<u8> { (0..len as u64).map(|k| unsafe { *((data + ((off + k) % size) as usize) as *const u8) }).collect() };
        while tail + 8 <= head {
            let h = copy(tail, 8);
            let ty = u32::from_ne_bytes(h[0..4].try_into().unwrap());
            let sz = u16::from_ne_bytes(h[6..8].try_into().unwrap()) as u64;
            if sz < 8 {
                break;
            }
            if ty == 2 {
                *lost += 1; // PERF_RECORD_LOST
            }
            if ty == 9 {
                let rec = copy(tail, sz as usize);
                if let Some(s) = parse(&rec, r.os_id) {
                    out.push(s);
                }
            }
            tail += sz;
        }
        unsafe { (*tail_p).store(tail, Ordering::Release) };
    }

    /// One PERF_RECORD_SAMPLE: header, ip, pid/tid, period, regs (abi + 8 regs), stack (size,
    /// bytes, dyn_size); walked on the copied stack.
    fn parse(rec: &[u8], os_id: u64) -> Option<Sample> {
        let u = |o: usize| -> Option<u64> { Some(u64::from_ne_bytes(rec.get(o..o + 8)?.try_into().ok()?)) };
        let mut o = 8 + 8 + 8;
        let weight = u(o)?;
        o += 8;
        let abi = u(o)?;
        o += 8;
        if abi == 0 {
            return None;
        }
        let g: Vec<u64> = (0..8).map(|k| u(o + k * 8).unwrap_or(0)).collect();
        o += 64;
        let size = u(o)? as usize;
        o += 8;
        let stack = rec.get(o..o + size)?;
        let dyn_size = u(o + size).unwrap_or(0) as usize;
        let stack = &stack[..dyn_size.min(size)];
        // bx, bp, sp, ip, r12, r13, r14, r15
        let mut regs = Regs { rbx: g[0], rbp: g[1], rsp: g[2], rip: g[3], r12: g[4], r13: g[5], r14: g[6], r15: g[7] };
        let sp = regs.rsp;
        let read = |a: u64| -> Option<u64> {
            let k = a.checked_sub(sp)? as usize;
            Some(u64::from_ne_bytes(stack.get(k..k + 8)?.try_into().ok()?))
        };
        let mut buf = [0u64; 96];
        let n = crate::inspect::threads::walk_regs_with(&mut regs, &mut buf, &read);
        Some(Sample { os_id, frames: buf[..n].to_vec(), weight, t: crate::clock::wall_ns() })
    }

    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64, ev: super::Ev, cancel: &dyn Fn() -> bool) -> Run {
        let mut notes = Vec::new();
        let mut ev = ev;
        let order = by_activity(&threads);
        let open_all = |ev: super::Ev, notes: &mut Vec<String>| -> Vec<Ring> {
            let mut rings = Vec::new();
            let (mut no_event, mut no_map) = (Vec::new(), Vec::new());
            let mut errno = 0;
            let mut pages = RING_PAGES;
            for &k in &order {
                let t = &threads[k];
                // once a ring had to shrink, the next ones start at its size
                match open(t.os_id, hz, ev, pages) {
                    Ok(r) => {
                        pages = r.pages;
                        rings.push(r);
                    }
                    Err(OpenErr::Event(e)) => {
                        errno = e;
                        no_event.push(t.name.clone());
                    }
                    Err(OpenErr::Map) => no_map.push(t.name.clone()),
                }
            }
            let list = |v: &[String]| v.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
            if !no_map.is_empty() && !rings.is_empty() {
                notes.push(format!("{} of {} threads not sampled (the busiest {} were): the perf locked-memory budget is spent (RLIMIT_MEMLOCK + perf_event_mlock_kb, shared with this user's other perf jobs; rings of {} KB); not sampled: {}", no_map.len(), threads.len(), rings.len(), pages * PAGE / 1024, list(&no_map)));
            } else if !no_map.is_empty() {
                notes.push(format!("no thread could be sampled: no perf ring maps (locked-memory budget spent: RLIMIT_MEMLOCK + perf_event_mlock_kb, shared with this user's other perf jobs)"));
            }
            if !no_event.is_empty() && rings.is_empty() && ev == super::Ev::Cpu {
                notes.push(format!("no thread could be sampled: perf_event_open failed (errno {}; perf_event_paranoid, seccomp or a container)", errno));
            } else if !no_event.is_empty() && !rings.is_empty() {
                notes.push(format!("{} threads not sampled (perf_event_open errno {}: they exited?): {}", no_event.len(), errno, list(&no_event)));
            }
            rings
        };
        let mut rings = open_all(ev, &mut notes);
        if rings.is_empty() && ev != super::Ev::Cpu && !threads.is_empty() {
            notes.push(format!("{} cannot be sampled here (no hardware PMU access for this process: perf_event_paranoid, a VM); CPU time instead", ev.name()));
            ev = super::Ev::Cpu;
            rings = open_all(ev, &mut notes);
        }
        let mut samples = Vec::new();
        let mut lost = 0u64;
        // small rings hold a few samples: drained more often
        let tick = if rings.iter().any(|r| r.pages < 64) { 2 } else { 10 };
        let t0 = std::time::Instant::now();
        crate::inspect::activity::phase("sampling (perf rings)", 0);
        while t0.elapsed().as_secs_f64() < secs && !cancel() {
            std::thread::sleep(std::time::Duration::from_millis(tick));
            for r in &rings {
                drain(r, &mut samples, &mut lost);
            }
        }
        for r in &rings {
            drain(r, &mut samples, &mut lost);
            unsafe {
                munmap(r.base as *mut u8, (1 + r.pages) * PAGE);
                close(r.fd);
            }
        }
        let secs = t0.elapsed().as_secs_f64().min(secs);
        crate::inspect::activity::phase("running", 0);
        if ev == super::Ev::Cpu {
            for s in samples.iter_mut() {
                s.weight = 1;
            }
        }
        Run { samples, threads, secs, hz, lost, notes, ev }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Run, Sample};

    /// Each round suspends the picked threads that are on a CPU, one at a time, walks it into a
    /// fixed array and resumes it before anything is allocated or locked (a suspended thread may
    /// hold malloc's lock). The capture lock is taken per round, so `stacks` and `why-hung` get
    /// in between.
    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64, ev: super::Ev, cancel: &dyn Fn() -> bool) -> Run {
        let mut notes = Vec::new();
        if ev != super::Ev::Cpu {
            notes.push(format!("{} cannot be sampled on macOS without root (per-thread hardware sampling is privileged); CPU time instead -- `counters` gives this process's total instructions and cycles", ev.name()));
        }
        let mut busy_rounds = 0u64;
        let mut samples = Vec::with_capacity(((secs * hz as f64) as usize).min(1 << 16));
        let mut raw = crate::inspect::threads::Raw::new();
        let period = std::time::Duration::from_micros(1_000_000 / hz.max(1));
        let t0 = std::time::Instant::now();
        let mut last_cpu: Vec<u64> = threads.iter().map(|t| t.cpu_ns).collect();
        let mut live = crate::inspect::threads::list();
        // which threads ran in the last 50-ms look (CPU time moved, or on a CPU then): only those
        // are checked each round. The check itself (`running`) is live: a filter on the look's
        // own snapshot let a thread that runs in bursts (a UI thread: a few ms per frame) give
        // one sample per look at most (52 samples in 8 s of a thread that used 1.7 s of CPU)
        let mut ran: Vec<bool> = vec![true; threads.len()];
        let mut next_list = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < secs && !cancel() {
            let tick = std::time::Instant::now();
            if tick >= next_list {
                live = crate::inspect::threads::list();
                next_list = tick + std::time::Duration::from_millis(50);
                for (k, t) in threads.iter().enumerate() {
                    ran[k] = live.iter().find(|x| x.os_id == t.os_id).is_some_and(|now| {
                        let moved = now.cpu_ns != last_cpu[k] || now.state == "running";
                        last_cpu[k] = now.cpu_ns;
                        moved
                    });
                }
            }
            let _g = match crate::inspect::capture::lock("prof", 1000) {
                Ok(g) => g,
                Err(e) => {
                    busy_rounds += 1;
                    if busy_rounds == 1 {
                        notes.push(e);
                    }
                    continue;
                }
            };
            for (k, t) in threads.iter().enumerate() {
                let Some(now) = live.iter().find(|x| x.os_id == t.os_id) else { continue };
                // CPU samples only: a thread that ran lately and is on a CPU right now
                if !ran[k] || !crate::inspect::threads::running(now) {
                    continue;
                }
                if crate::inspect::threads::capture(now, &mut raw).is_ok() && raw.n > 0 {
                    samples.push(Sample { os_id: t.os_id, frames: raw.frames().to_vec(), weight: 1, t: crate::clock::wall_ns() });
                }
            }
            drop(_g);
            let spent = tick.elapsed();
            if spent < period {
                crate::inspect::activity::phase("sampling (between rounds)", 0);
                std::thread::sleep(period - spent);
            }
        }
        if busy_rounds > 0 {
            notes.push(format!("{} sampling round(s) skipped: the capture lock was busy", busy_rounds));
        }
        crate::inspect::activity::phase("running", 0);
        let secs = t0.elapsed().as_secs_f64().min(secs);
        Run { samples, threads, secs, hz, lost: 0, notes, ev: super::Ev::Cpu }
    }
}

#[cfg(any(windows, target_os = "android"))]
mod imp {
    use super::Run;
    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64, _ev: super::Ev, _cancel: &dyn Fn() -> bool) -> Run {
        Run { samples: Vec::new(), threads, secs, hz, lost: 0, notes: vec!["no CPU sampler on this target".into()], ev: super::Ev::Cpu }
    }
}

// ------------------------------------------------------------------ reports

/// What a sample's thread was doing, from its frames (innermost first): compiling (inside the
/// JIT's compile entry or an owner request), Relax runtime, program code, or system code.
fn activity(names: &[super::Frame]) -> &'static str {
    let mut jit_seen = false;
    for f in names {
        if f.module.starts_with("jit") {
            jit_seen = true;
            continue;
        }
        let n = f.name.as_str();
        if !jit_seen && (n.contains("relax_compiler::jit::rt_compile") || n.contains("compile_slot") || n.contains("relax_compiler::owner") || n.contains("relax_codegen::") || n.contains("relax_types::") || n.contains("relax_front::")) {
            return "compile";
        }
    }
    match names.first() {
        // a thunk, lazy stub or veneer between a call and its callee: what calls cost to reach
        Some(f) if f.module.starts_with("jit/glue") => "JIT call glue (thunks, stubs, veneers)",
        Some(f) if f.module.starts_with("jit") => "program",
        Some(f) if f.module == "makepad" => {
            if jit_seen {
                "makepad run runtime / compiled-in crates"
            } else {
                "makepad run"
            }
        }
        _ => {
            if jit_seen {
                "system (called by the program)"
            } else {
                "system"
            }
        }
    }
}

fn pick(sel: &str) -> Box<dyn Fn(&super::threads::Thread) -> bool> {
    let sel = sel.to_string();
    let main = crate::jit::MAIN_TID.load(std::sync::atomic::Ordering::Relaxed);
    let owner = crate::owner::owner_tid();
    let pid = std::process::id() as u64;
    Box::new(move |t: &super::threads::Thread| match sel.as_str() {
        "all" | "" => true,
        "main" => (t.handle != 0 && t.handle == main) || t.os_id == pid,
        "owner" => t.handle != 0 && t.handle == owner,
        "compiler" => t.name.starts_with("run-") || t.handle == owner,
        "program" => !t.name.starts_with("run-") && !t.name.starts_with("gandalf") && t.handle != owner,
        s => s.split(',').any(|s| super::tid_of(s).is_some_and(|id| id == t.os_id) || t.name.starts_with(s)),
    })
}

/// `prof [s=3] [hz=200] [threads=all|main|owner|program|compiler|<name>] [n=15] [save=1]`
pub fn cmd_prof(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.2, 30.0);
    let hz = req.num("hz", 200).clamp(10, 2000);
    let n = req.num("n", 15) as usize;
    let ev = match Ev::parse(req.get("ev").unwrap_or("cpu")) {
        Ok(e) => e,
        Err(e) => return out.err(&e),
    };
    let gone = out.gone();
    let run = collect_ev(secs, hz, ev, &*pick(req.get("threads").unwrap_or("all")), &*gone);
    if gone() {
        return;
    }
    if matches!(req.get("view"), Some("tree" | "callers" | "flame")) {
        return super::calltree::report(&run, req, out);
    }
    report(&run, n, req.get("save") == Some("1"), out);
}

/// Symbolised frames per sample (JIT names by the owner, with a wait; host names from the
/// images' symbol tables).
fn names(run: &Run, partial: &mut Vec<String>) -> Vec<Vec<super::Frame>> {
    let raw: Vec<Vec<u64>> = run.samples.iter().map(|s| s.frames.clone()).collect();
    super::symbolise_within(&raw, partial, 3000)
}

fn report(run: &Run, n: usize, save: bool, out: &mut Out) {
    let mut partial = Vec::new();
    if run.lost > 0 {
        partial.push(format!("{} sample batches lost (ring overflow)", run.lost));
    }
    partial.extend(run.notes.iter().cloned());
    let syms = names(run, &mut partial);
    let total = run.samples.len().max(1);
    let ms_per = 1000.0 / run.hz as f64;
    out.measured(&format!("CPU samples: {} over {:.1} s at {} Hz of CPU time per thread ({:.1} ms each)", run.samples.len(), run.secs, run.hz, ms_per), "perf_event / thread suspend (no owner)", &partial);
    // per thread
    let mut per: Vec<(u64, usize)> = Vec::new();
    for s in &run.samples {
        match per.iter_mut().find(|x| x.0 == s.os_id) {
            Some(x) => x.1 += 1,
            None => per.push((s.os_id, 1)),
        }
    }
    per.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let tname = |id: u64| run.threads.iter().find(|t| t.os_id == id).map(|t| t.name.clone()).unwrap_or_default();
    // CPU time per thread from the OS counters over the window (samples are weighted by CPU
    // time, but their count is not the CPU time: the sampler backs off on idle threads)
    let now = super::threads::list();
    let cpu_ms = |id: u64| -> f64 {
        let b = run.threads.iter().find(|t| t.os_id == id).map_or(0, |t| t.cpu_ns);
        now.iter().find(|t| t.os_id == id).map_or(0, |t| t.cpu_ns).saturating_sub(b) as f64 / 1e6
    };
    out.line(&format!("{{\"window_s\":{:.2},\"hz\":{},\"samples\":{}}}", run.secs, run.hz, run.samples.len()));
    let mut by_cpu: Vec<(u64, f64, usize)> = run.threads.iter().map(|t| (t.os_id, cpu_ms(t.os_id), per.iter().find(|x| x.0 == t.os_id).map_or(0, |x| x.1))).filter(|x| x.1 >= 1.0 || x.2 > 0).collect();
    by_cpu.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.2.cmp(&a.2)));
    let threads: Vec<String> = by_cpu
        .iter()
        .take(12)
        .map(|(id, ms, c)| format!("{{\"thread\":{},\"id\":\"{}\",\"cpu_ms\":{:.0},\"pct_of_samples\":{:.1}}}", json_str(&tname(*id)), super::tid_label(*id), ms, *c as f64 * 100.0 / total as f64))
        .collect();
    let rest: f64 = by_cpu.iter().skip(12).map(|x| x.1).sum();
    // the busiest thread off its CPU: where it waits (the samples only see it running)
    if let Some(&(id, ms, _)) = by_cpu.first() {
        let wall = run.secs * 1000.0;
        if ms < wall * 0.8 && ms > 0.0 {
            let (seen, n) = off_cpu_stacks(id);
            if let Some((k, c)) = seen.first() {
                out.line(&format!("{{\"off_cpu\":{{\"tid\":\"{}\",\"pct\":{:.0},\"stack\":{},\"seen\":{},\"of\":{}}}}}", super::tid_label(id), (wall - ms) * 100.0 / wall.max(1.0), json_str(&short(k, 240)), c, n));
            }
        }
    }
    out.line(&format!("{{\"threads\":[{}],\"other_threads\":{},\"other_cpu_ms\":{:.0}}}", threads.join(","), by_cpu.len().saturating_sub(12), rest));
    let _ = ms_per;
    // activity split
    let mut act: Vec<(&str, usize)> = Vec::new();
    for s in &syms {
        let a = activity(s);
        match act.iter_mut().find(|x| x.0 == a) {
            Some(x) => x.1 += 1,
            None => act.push((a, 1)),
        }
    }
    act.sort_by(|a, b| b.1.cmp(&a.1));
    let a: Vec<String> = act.iter().map(|(k, c)| format!("{}:{:.1}", json_str(k), *c as f64 * 100.0 / total as f64)).collect();
    out.line(&format!("{{\"activity_pct\":{{{}}}}}", a.join(",")));
    // self and total by fn
    let key = |f: &super::Frame| if f.module.starts_with("jit") { format!("{} [{}]", f.name, f.module) } else { format!("{} [{}]", f.name, f.module) };
    let mut selfc: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut totc: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut at: std::collections::HashMap<String, std::collections::HashMap<String, usize>> = std::collections::HashMap::new();
    let mut rep: std::collections::HashMap<String, &super::Frame> = std::collections::HashMap::new();
    let mut by_thread: std::collections::HashMap<String, std::collections::HashMap<u64, usize>> = std::collections::HashMap::new();
    for (s, s_os) in syms.iter().zip(run.samples.iter().map(|x| x.os_id)) {
        if let Some(f) = s.first() {
            rep.entry(key(f)).or_insert(f);
            *selfc.entry(key(f)).or_default() += 1;
            *by_thread.entry(key(f)).or_default().entry(s_os).or_default() += 1;
            if !f.at.is_empty() {
                *at.entry(key(f)).or_default().entry(f.at.clone()).or_default() += 1;
            }
        }
        let mut seen: Vec<String> = Vec::new();
        for f in s {
            let k = key(f);
            if !seen.contains(&k) {
                *totc.entry(k.clone()).or_default() += 1;
                seen.push(k);
            }
        }
    }
    let mut top: Vec<(&String, &usize)> = selfc.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (k, c) in top.iter().take(n) {
        let line = at.get(*k).and_then(|m| m.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))).map(|(l, lc)| format!(",\"hottest_line\":{},\"line_pct\":{:.0}", json_str(l), *lc as f64 * 100.0 / **c as f64)).unwrap_or_default();
        // the frame's own fields: the fn, its code (tier), its slot and, for an inlined copy, the
        // fn whose code it is in
        let f = rep.get(*k).copied();
        let mut fields = String::new();
        if let Some(f) = f {
            fields.push_str(&format!(",\"in\":{}", json_str(f.module.split(" inlined").next().unwrap_or(""))));
            if let Some(sl) = f.slot {
                fields.push_str(&format!(",\"slot\":{}", sl));
            }
            if let Some((into, _)) = f.levels.last() {
                fields.push_str(&format!(",\"inlined_into\":{}", json_str(&short(into, 160))));
            }
        }
        if let Some((t, _)) = by_thread.get(*k).and_then(|m| m.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))) {
            fields.push_str(&format!(",\"thread\":{},\"tid\":\"{}\"", json_str(&tname(*t)), super::tid_label(*t)));
        }
        let name = f.map_or_else(|| short(k, 160), |f| short(&f.name, 160));
        out.line(&format!("{{\"self_pct\":{:.1},\"total_pct\":{:.1},\"fn\":{}{}{}}}", **c as f64 * 100.0 / total as f64, *totc.get(*k).unwrap_or(c) as f64 * 100.0 / total as f64, json_str(&name), fields, line));
    }
    out.line(&format!("{{\"fns_not_shown\":{},\"not_shown_self_pct\":{:.1}}}", top.len().saturating_sub(n), top.iter().skip(n).map(|x| *x.1).sum::<usize>() as f64 * 100.0 / total as f64));
    let mut tops: Vec<(&String, &usize)> = totc.iter().filter(|x| *x.1 * 100 >= total * 10).collect();
    tops.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let t: Vec<String> = tops.iter().take(n).map(|(k, c)| format!("[{:.1},{}]", **c as f64 * 100.0 / total as f64, json_str(&short(k, 120)))).collect();
    out.line(&format!("{{\"inclusive_pct_over_10\":[{}]}}", t.join(",")));
    if save {
        // folded stacks (flame graph input): thread;outer;...;inner count
        let mut folded: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for (s, f) in run.samples.iter().zip(&syms) {
            let mut parts: Vec<String> = vec![tname(s.os_id)];
            parts.extend(f.iter().rev().map(|x| x.name.replace(';', ":")));
            *folded.entry(parts.join(";")).or_default() += 1;
        }
        let path = std::env::temp_dir().join(format!("makepad-run-inspect-{}-prof-{}.folded", std::process::id(), now_ms()));
        let text: String = folded.iter().map(|(k, v)| format!("{} {}\n", k, v)).collect();
        match std::fs::write(&path, text) {
            Ok(()) => out.line(&format!("{{\"saved\":{},\"format\":\"folded stacks (flamegraph.pl / speedscope)\"}}", json_str(&path.display().to_string()))),
            Err(e) => out.err(&format!("cannot write {}: {}", path.display(), e)),
        }
    }
    let _ = frame_json;
}

/// Where thread `os_id` is when it is not on a CPU: its stack a few times (15 ms apart), the
/// distinct tops ("__semwait_signal in std::sys::time::sleep from main (file:line)") most seen first.
fn off_cpu_stacks(os_id: u64) -> (Vec<(String, usize)>, usize) {
    let is_it = |t: &super::threads::Thread| t.os_id == os_id;
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut caps_n = 0;
    for _ in 0..5 {
        if let Ok(c) = super::capture_all(&is_it) {
            if let Some(c) = c.first() {
                let mut p2 = Vec::new();
                let f: Vec<Vec<u64>> = vec![c.frames.iter().copied().take(24).collect()];
                let sy = super::symbolise_within(&f, &mut p2, 300);
                let fr = &sy[0];
                let top = fr.first().map(|f| f.name.clone()).unwrap_or_default();
                let prog = fr.iter().find(|f| super::is_program_frame(f)).map(|f| format!(" from {}{}", f.name, if f.at.is_empty() { String::new() } else { format!(" ({})", f.at) })).unwrap_or_default();
                let jit = fr.iter().find(|f| f.module.starts_with("jit")).map(|f| format!(" in {}", f.name)).unwrap_or_default();
                let k = format!("{}{}{}", top, jit, prog);
                caps_n += 1;
                match seen.iter_mut().find(|x| x.0 == k) {
                    Some(x) => x.1 += 1,
                    None => seen.push((k, 1)),
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    seen.sort_by(|a, b| b.1.cmp(&a.1));
    (seen, caps_n)
}

/// `why-slow [s=3]`: ranked causes of where the main thread's time goes: its CPU split (program,
/// compiles, Relax runtime, system), the hot program fns, and the owner's service while it ran.
pub fn cmd_why_slow(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.5, 30.0);
    let cpu0 = super::threads::list();
    let busy0 = crate::owner::BUSY_NS.iter().map(|b| b.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>();
    let wait0 = crate::owner::MAIN_WAIT_MAX.load(std::sync::atomic::Ordering::Relaxed);
    let gone = out.gone();
    let run = collect(secs, 250, &*pick("all"), &*gone);
    if gone() {
        return;
    }
    let cpu1 = super::threads::list();
    let busy1 = crate::owner::BUSY_NS.iter().map(|b| b.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>();
    let mut partial = run.notes.clone();
    let syms = names(&run, &mut partial);
    out.measured(&format!("{:.1} s: CPU samples of every thread at 250 Hz, thread CPU times, owner service counters", run.secs), "perf_event / thread suspend + owner counters", &partial);
    let pid = std::process::id() as u64;
    let main_h = crate::jit::MAIN_TID.load(std::sync::atomic::Ordering::Relaxed);
    let main_id = run.threads.iter().find(|t| (t.handle != 0 && t.handle == main_h) || t.os_id == pid).map(|t| t.os_id).unwrap_or(pid);
    let cpu_of = |l: &[super::threads::Thread], id: u64| l.iter().find(|t| t.os_id == id).map_or(0, |t| t.cpu_ns);
    let main_cpu_ms = cpu_of(&cpu1, main_id).saturating_sub(cpu_of(&cpu0, main_id)) as f64 / 1e6;
    let wall_ms = run.secs * 1000.0;
    let mut findings: Vec<(f64, String)> = Vec::new();
    // main thread activity
    let main_samples: Vec<&Vec<super::Frame>> = run.samples.iter().zip(&syms).filter(|(s, _)| s.os_id == main_id).map(|x| x.1).collect();
    // samples are weighted by CPU time, not one per period: shares of main's samples times its
    // measured CPU time
    let ms_per = main_cpu_ms / main_samples.len().max(1) as f64;
    let mut act: Vec<(&str, usize)> = Vec::new();
    for s in &main_samples {
        let a = activity(s);
        match act.iter_mut().find(|x| x.0 == a) {
            Some(x) => x.1 += 1,
            None => act.push((a, 1)),
        }
    }
    let busy_pct = main_cpu_ms * 100.0 / wall_ms.max(1.0);
    findings.push((main_cpu_ms, format!("main thread busy {:.0}% of the time ({:.0} ms CPU in {:.0} ms)", busy_pct, main_cpu_ms, wall_ms)));
    for (a, c) in &act {
        let ms = *c as f64 * ms_per;
        findings.push((ms * 0.99 - 0.01, format!("main: {:.0} ms in {}", ms, a)));
    }
    // hot program fns on main
    let mut hot: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &main_samples {
        if let Some(f) = s.iter().find(|f| f.module.starts_with("jit")) {
            let name = match f.levels.last() {
                Some((into, _)) if into != &f.name => format!("{} (inlined into {})", f.name, into),
                _ => f.name.clone(),
            };
            *hot.entry(if f.at.is_empty() { name } else { format!("{} ({})", name, f.at) }).or_default() += 1;
        }
    }
    // the program fn whose code the samples are in (its inlined copies counted with it)
    let mut phys: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &main_samples {
        // the innermost frame whose physical fn (the code it is in) is the program's
        if let Some(n) = s.iter().filter(|f| f.module.starts_with("jit")).map(|f| f.levels.last().map_or(f.name.clone(), |l| l.0.clone())).find(|n| !super::is_std_fn(n) && !n.starts_with("jit 0x") && !n.starts_with("slot#") && !n.starts_with('[')) {
            *phys.entry(n).or_default() += 1;
        }
    }
    if let Some((n, c)) = phys.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) {
        findings.push((main_cpu_ms - 0.002, format!("main's hottest program fn (with the code inlined into it): {} {:.0} ms = {:.0}% of main's CPU", n, *c as f64 * ms_per, *c as f64 * 100.0 / main_samples.len().max(1) as f64)));
    }
    let mut hv: Vec<(String, usize)> = hot.into_iter().collect();
    hv.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (k, c) in hv.iter().take(5) {
        findings.push((*c as f64 * ms_per, format!("main hot: {} ({:.0} ms)", short(k, 200), *c as f64 * ms_per)));
    }
    // the owner's service in the window, by class
    let names = crate::owner::CLASS_NAMES;
    for k in 0..busy1.len() {
        let ms = busy1[k].saturating_sub(busy0[k]) as f64 / 1e6;
        if ms >= 1.0 {
            findings.push((ms * 0.5, format!("owner served {} requests for {:.0} ms", names[k], ms)));
        }
    }
    let wait1 = crate::owner::MAIN_WAIT_MAX.load(std::sync::atomic::Ordering::Relaxed);
    if wait1 > wait0 {
        findings.push((wait1 as f64 / 1e6, format!("main waited up to {:.1} ms for the owner (new maximum, behind {})", wait1 as f64 / 1e6, crate::owner::main_wait_behind())));
    }
    // other busy threads
    for t in &cpu1 {
        if t.os_id == main_id {
            continue;
        }
        let ms = t.cpu_ns.saturating_sub(cpu_of(&cpu0, t.os_id)) as f64 / 1e6;
        if ms > wall_ms * 0.3 {
            findings.push((ms * 0.3, format!("thread {} ({}) used {:.0} ms CPU", t.name, super::tid_label(t.os_id), ms)));
        }
    }
    // where main is when it is not on a CPU: a few stack captures (the samples only see it running)
    let off = (wall_ms - main_cpu_ms).max(0.0);
    if off > wall_ms * 0.2 {
        let (seen, caps_n) = off_cpu_stacks(main_id);
        if let Some((k, c)) = seen.first() {
            findings.push((main_cpu_ms - 0.001, format!("main off CPU {:.0}% of the time ({:.0} ms); its stack then ({} of {} captures): {}", off * 100.0 / wall_ms.max(1.0), off, c, caps_n, short(k, 240))));
        }
    }
    findings.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    for (i, (_, f)) in findings.iter().take(req.num("n", 12) as usize).enumerate() {
        out.line(&format!("{{\"rank\":{},\"finding\":{}}}", i + 1, json_str(f)));
    }
    out.line("{\"more\":[\"prof threads=main s=5\",\"prof save=1\",\"why-hung\",\"stacks thread=main\"]}");
}

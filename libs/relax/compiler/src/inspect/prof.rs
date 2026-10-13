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
}

pub struct Run {
    pub samples: Vec<Sample>,
    pub threads: Vec<super::threads::Thread>,
    pub secs: f64,
    pub hz: u64,
    pub lost: u64,
}

/// Collects samples for `secs` at `hz` per thread of CPU time, from the threads `want` picks.
pub fn collect(secs: f64, hz: u64, want: &dyn Fn(&super::threads::Thread) -> bool) -> Run {
    super::cfi::refresh();
    let skip = super::SERVER_TIDS.lock().unwrap().clone();
    let mut threads = super::threads::list();
    super::name_handles(&mut threads);
    threads.retain(|t| !skip.contains(&t.os_id) && want(t) && !t.name.starts_with("run-inspect") || t.name.starts_with("makepad-remote"));
    imp::collect(threads, secs, hz)
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
    const RING_PAGES: usize = 256;
    const STACK_COPY: u32 = 16384;
    /// perf regs (x86): bx 1, bp 6, sp 7, ip 8, r12..r15 20..23 -- ascending order in a sample
    const REGS_MASK: u64 = (1 << 1) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 20) | (1 << 21) | (1 << 22) | (1 << 23);

    struct Ring {
        fd: i32,
        base: usize,
        os_id: u64,
    }

    fn open(tid: u64, hz: u64) -> Option<Ring> {
        let mut attr = [0u8; 128];
        let put = |a: &mut [u8; 128], off: usize, v: &[u8]| a[off..off + v.len()].copy_from_slice(v);
        put(&mut attr, 0, &1u32.to_ne_bytes()); // PERF_TYPE_SOFTWARE
        put(&mut attr, 4, &128u32.to_ne_bytes());
        put(&mut attr, 8, &1u64.to_ne_bytes()); // PERF_COUNT_SW_TASK_CLOCK
        put(&mut attr, 16, &(1_000_000_000 / hz.max(1)).to_ne_bytes()); // period, ns of CPU time
        put(&mut attr, 24, &(1u64 | 2 | 0x1000 | 0x2000).to_ne_bytes()); // IP | TID | REGS_USER | STACK_USER
        put(&mut attr, 40, &((1u64 << 5) | (1u64 << 6)).to_ne_bytes()); // exclude_kernel | exclude_hv
        put(&mut attr, 80, &REGS_MASK.to_ne_bytes());
        put(&mut attr, 88, &STACK_COPY.to_ne_bytes());
        let fd = unsafe { syscall(298, attr.as_ptr(), tid as i32, -1i32, -1i32, 0u64) } as i32;
        if fd < 0 {
            return None;
        }
        let len = (1 + RING_PAGES) * PAGE;
        let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 1, fd, 0) };
        if p as isize == -1 {
            unsafe { close(fd) };
            return None;
        }
        Some(Ring { fd, base: p as usize, os_id: tid })
    }

    fn drain(r: &Ring, out: &mut Vec<Sample>, lost: &mut u64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        let data = r.base + PAGE;
        let size = (RING_PAGES * PAGE) as u64;
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

    /// One PERF_RECORD_SAMPLE: header, ip, pid/tid, regs (abi + 8 regs), stack (size, bytes,
    /// dyn_size); walked on the copied stack.
    fn parse(rec: &[u8], os_id: u64) -> Option<Sample> {
        let u = |o: usize| -> Option<u64> { Some(u64::from_ne_bytes(rec.get(o..o + 8)?.try_into().ok()?)) };
        let mut o = 8 + 8 + 8;
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
        Some(Sample { os_id, frames: buf[..n].to_vec() })
    }

    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64) -> Run {
        let rings: Vec<Ring> = threads.iter().filter_map(|t| open(t.os_id, hz)).collect();
        let mut samples = Vec::new();
        let mut lost = 0u64;
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            std::thread::sleep(std::time::Duration::from_millis(10));
            for r in &rings {
                drain(r, &mut samples, &mut lost);
            }
        }
        for r in &rings {
            drain(r, &mut samples, &mut lost);
            unsafe {
                munmap(r.base as *mut u8, (1 + RING_PAGES) * PAGE);
                close(r.fd);
            }
        }
        Run { samples, threads, secs, hz, lost }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Run, Sample};

    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64) -> Run {
        let _g = crate::inspect::CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        let mut samples = Vec::new();
        let mut raw = crate::inspect::threads::Raw::new();
        let period = std::time::Duration::from_micros(1_000_000 / hz.max(1));
        let t0 = std::time::Instant::now();
        let mut last_cpu: Vec<u64> = threads.iter().map(|t| t.cpu_ns).collect();
        let mut live = crate::inspect::threads::list();
        let mut next_list = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < secs {
            let tick = std::time::Instant::now();
            // which threads ran since the last look (every 50 ms): only those are suspended
            if tick >= next_list {
                live = crate::inspect::threads::list();
                next_list = tick + std::time::Duration::from_millis(50);
            }
            for (k, t) in threads.iter().enumerate() {
                let Some(now) = live.iter().find(|x| x.os_id == t.os_id) else { continue };
                // CPU samples only: a thread that ran lately and is on a CPU right now
                if now.cpu_ns == last_cpu[k] && now.state != "running" {
                    continue;
                }
                last_cpu[k] = now.cpu_ns;
                if !crate::inspect::threads::running(now) {
                    continue;
                }
                if crate::inspect::threads::capture(now, &mut raw).is_ok() && raw.n > 0 {
                    samples.push(Sample { os_id: t.os_id, frames: raw.frames().to_vec() });
                }
            }
            let spent = tick.elapsed();
            if spent < period {
                std::thread::sleep(period - spent);
            }
        }
        Run { samples, threads, secs, hz, lost: 0 }
    }
}

#[cfg(any(windows, target_os = "android"))]
mod imp {
    use super::Run;
    pub fn collect(threads: Vec<crate::inspect::threads::Thread>, secs: f64, hz: u64) -> Run {
        Run { samples: Vec::new(), threads, secs, hz, lost: 0 }
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
        s => s.split(',').any(|s| s.trim_start_matches('T').parse::<u64>().map_or(false, |id| id == t.os_id) || t.name.starts_with(s)),
    })
}

/// `prof [s=3] [hz=200] [threads=all|main|owner|program|compiler|<name>] [n=15] [save=1]`
pub fn cmd_prof(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.2, 30.0);
    let hz = req.num("hz", 200).clamp(10, 2000);
    let n = req.num("n", 15) as usize;
    let run = collect(secs, hz, &*pick(req.get("threads").unwrap_or("all")));
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
    let threads: Vec<String> = per
        .iter()
        .take(12)
        .map(|(id, c)| format!("{{\"thread\":{},\"id\":\"T{}\",\"cpu_ms\":{:.0},\"pct_of_samples\":{:.1}}}", json_str(&tname(*id)), id, *c as f64 * ms_per, *c as f64 * 100.0 / total as f64))
        .collect();
    out.line(&format!("{{\"threads\":[{}]}}", threads.join(",")));
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
    for s in &syms {
        if let Some(f) = s.first() {
            *selfc.entry(key(f)).or_default() += 1;
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
        out.line(&format!("{{\"self_pct\":{:.1},\"total_pct\":{:.1},\"fn\":{}{}}}", **c as f64 * 100.0 / total as f64, *totc.get(*k).unwrap_or(c) as f64 * 100.0 / total as f64, json_str(&short(k, 160)), line));
    }
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

/// `why-slow [s=3]`: ranked causes of where the main thread's time goes: its CPU split (program,
/// compiles, Relax runtime, system), the hot program fns, and the owner's service while it ran.
pub fn cmd_why_slow(req: &Req, out: &mut Out) {
    let secs = (req.num("s", 3) as f64).clamp(0.5, 30.0);
    let cpu0 = super::threads::list();
    let busy0 = crate::owner::BUSY_NS.iter().map(|b| b.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>();
    let wait0 = crate::owner::MAIN_WAIT_MAX.load(std::sync::atomic::Ordering::Relaxed);
    let run = collect(secs, 250, &*pick("all"));
    let cpu1 = super::threads::list();
    let busy1 = crate::owner::BUSY_NS.iter().map(|b| b.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>();
    let mut partial = Vec::new();
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
    let ms_per = 1000.0 / 250.0;
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
        findings.push((ms, format!("main: {:.0} ms in {}", ms, a)));
    }
    // hot program fns on main
    let mut hot: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in &main_samples {
        if let Some(f) = s.iter().find(|f| f.module.starts_with("jit")) {
            *hot.entry(if f.at.is_empty() { f.name.clone() } else { format!("{} ({})", f.name, f.at) }).or_default() += 1;
        }
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
            findings.push((ms * 0.3, format!("thread {} (T{}) used {:.0} ms CPU", t.name, t.os_id, ms)));
        }
    }
    findings.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    for (i, (_, f)) in findings.iter().take(req.num("n", 12) as usize).enumerate() {
        out.line(&format!("{{\"rank\":{},\"finding\":{}}}", i + 1, json_str(f)));
    }
    out.line("{\"more\":[\"prof threads=main s=5\",\"prof save=1\",\"why-hung\",\"stacks thread=main\"]}");
}

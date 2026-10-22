//! `vsnative [native=<bin>] [args="..."] [s=5] [warm_ms=3000] [ev=instructions|cycles|cpu] [n=20]`:
//! the same program as a rustc release build, run next to this JIT process for the same window,
//! both sampled with the same event, and diffed per fn: events per second here and there, the
//! ratio, each fn's share -- the JIT-vs-native table per fn (which fns cost more under the JIT and
//! by how much), built in. The native process is a child of this one (same environment: hidden
//! windows, mute, headless backends), sampled by perf_event on each of its threads (leaf pc only),
//! its pcs named by its own symbol table (inspect::natbin).
//!
//! Per second of wall is the right unit for work at a fixed pace (playback, a frame-locked app,
//! a replayed input recording); two flat-out loops are compared by their shares (the `%` columns).
//! Linux only: macOS lets a process sample another one only with privileges.

use super::{Out, Req};

pub fn cmd_vsnative(req: &Req, out: &mut Out) {
    imp::run(req, out);
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Out, Req};
    use crate::inspect::short;
    use std::collections::HashMap;

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn munmap(a: *mut u8, len: usize) -> i32;
        fn close(fd: i32) -> i32;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_PERF_EVENT_OPEN: i64 = 298;
    #[cfg(target_arch = "aarch64")]
    const SYS_PERF_EVENT_OPEN: i64 = 241;
    const PAGE: usize = 4096;
    const RING_PAGES: usize = 64;

    struct Ring {
        fd: i32,
        base: usize,
    }

    /// Leaf-pc sampling of thread `tid` (another process's): ip + period only.
    fn open(tid: i32, ev: crate::inspect::prof::Ev, hz: u64) -> Option<Ring> {
        let mut attr = [0u8; 128];
        let put = |a: &mut [u8; 128], off: usize, v: &[u8]| a[off..off + v.len()].copy_from_slice(v);
        let flags = (1u64 << 5) | (1u64 << 6); // exclude_kernel | exclude_hv
        let (ty, config) = match ev {
            crate::inspect::prof::Ev::Cpu => (1u32, 1u64),
            crate::inspect::prof::Ev::Cycles => (0, 0),
            crate::inspect::prof::Ev::Instructions => (0, 1),
            crate::inspect::prof::Ev::CacheMisses => (0, 3),
            crate::inspect::prof::Ev::BranchMisses => (0, 5),
        };
        // a fixed period, the same as this process's own sampling (prof::Ev::period)
        put(&mut attr, 16, &ev.period(hz).to_ne_bytes());
        put(&mut attr, 0, &ty.to_ne_bytes());
        put(&mut attr, 4, &128u32.to_ne_bytes());
        put(&mut attr, 8, &config.to_ne_bytes());
        put(&mut attr, 24, &(1u64 | 0x100).to_ne_bytes()); // IP | PERIOD
        put(&mut attr, 40, &flags.to_ne_bytes());
        let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), tid, -1i32, -1i32, 0u64) } as i32;
        if fd < 0 {
            return None;
        }
        let len = (1 + RING_PAGES) * PAGE;
        let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 1, fd, 0) };
        if p as isize == -1 {
            unsafe { close(fd) };
            return None;
        }
        Some(Ring { fd, base: p as usize })
    }

    fn drain(r: &Ring, out: &mut Vec<(u64, u64)>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        let data = r.base + PAGE;
        let size = (RING_PAGES * PAGE) as u64;
        let head = unsafe { (*((r.base + 1024) as *const AtomicU64)).load(Ordering::Acquire) };
        let tail_p = (r.base + 1032) as *const AtomicU64;
        let mut tail = unsafe { (*tail_p).load(Ordering::Relaxed) };
        let byte = |off: u64| unsafe { *((data + (off % size) as usize) as *const u8) };
        let u64at = |off: u64| (0..8).fold(0u64, |a, k| a | (byte(off + k) as u64) << (8 * k));
        while tail + 8 <= head {
            let ty = (0..4).fold(0u32, |a, k| a | (byte(tail + k) as u32) << (8 * k));
            let sz = (byte(tail + 6) as u64) | (byte(tail + 7) as u64) << 8;
            if sz < 8 {
                break;
            }
            if ty == 9 && sz >= 24 {
                out.push((u64at(tail + 8), u64at(tail + 16)));
            }
            tail += sz;
        }
        unsafe { (*tail_p).store(tail, Ordering::Release) };
    }

    /// pc -> file virtual address of the child's executable: its mappings of the file, and the
    /// file's PT_LOAD segments (offset -> vaddr).
    struct Map {
        maps: Vec<(u64, u64, u64)>,
        loads: Vec<(u64, u64, u64)>,
    }

    impl Map {
        fn of(pid: u32, exe: &str) -> Map {
            let canon = std::fs::canonicalize(exe).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| exe.to_string());
            let mut maps = Vec::new();
            if let Ok(t) = std::fs::read_to_string(format!("/proc/{}/maps", pid)) {
                for l in t.lines() {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    if f.len() >= 6 && f[5] == canon {
                        if let Some((a, b)) = f[0].split_once('-') {
                            if let (Ok(a), Ok(b), Ok(o)) = (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16), u64::from_str_radix(f[2], 16)) {
                                maps.push((a, b, o));
                            }
                        }
                    }
                }
            }
            let mut loads = Vec::new();
            if let Ok(b) = std::fs::read(exe) {
                let u16a = |o: usize| b.get(o..o + 2).map_or(0, |x| u16::from_le_bytes([x[0], x[1]]) as u64);
                let u32a = |o: usize| b.get(o..o + 4).map_or(0, |x| u32::from_le_bytes(x.try_into().unwrap()) as u64);
                let u64a = |o: usize| b.get(o..o + 8).map_or(0, |x| u64::from_le_bytes(x.try_into().unwrap()));
                if b.len() > 64 && b[..4] == [0x7f, b'E', b'L', b'F'] {
                    let phoff = u64a(0x20) as usize;
                    let phsz = u16a(0x36) as usize;
                    let phn = u16a(0x38) as usize;
                    for k in 0..phn {
                        let p = phoff + k * phsz;
                        if u32a(p) == 1 {
                            loads.push((u64a(p + 8), u64a(p + 16), u64a(p + 32)));
                        }
                    }
                }
            }
            Map { maps, loads }
        }
        fn vaddr(&self, pc: u64) -> Option<u64> {
            let &(a, _, o) = self.maps.iter().find(|m| pc >= m.0 && pc < m.1)?;
            let foff = pc - a + o;
            let &(po, pv, _) = self.loads.iter().find(|l| foff >= l.0 && foff < l.0 + l.2)?;
            Some(foff - po + pv)
        }
    }

    pub fn run(req: &Req, out: &mut Out) {
        let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(5.0).clamp(0.5, 60.0);
        let warm = req.num("warm_ms", 3000).min(120_000);
        let hz = req.num("hz", 1000).clamp(50, 4000);
        let n = req.num("n", 20) as usize;
        let ev = match crate::inspect::prof::Ev::parse(req.get("ev").unwrap_or("instructions")) {
            Ok(e) => e,
            Err(e) => return out.err(&e),
        };
        let native = match req.get("native").map(|s| s.to_string()).map(Ok).unwrap_or_else(|| crate::inspect::asm::native_for(req)) {
            Ok(p) => p,
            Err(e) => return out.err(&e),
        };
        // the program's own arguments (after `--` on makepad run's command line), unless args= says
        let args: Vec<String> = match req.get("args") {
            Some(a) => a.split(',').map(|s| s.to_string()).filter(|s| !s.is_empty()).collect(),
            None => {
                let all: Vec<String> = std::env::args().collect();
                all.iter().position(|a| a == "--").map(|k| all[k + 1..].to_vec()).unwrap_or_default()
            }
        };
        let log = std::env::temp_dir().join(format!("makepad-vsnative-{}.log", std::process::id()));
        let f = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return out.err(&format!("cannot write {}: {}", log.display(), e)),
        };
        let f2 = f.try_clone().ok();
        let mut cmd = std::process::Command::new(&native);
        cmd.args(&args).stdin(std::process::Stdio::null()).stdout(f);
        if let Some(f2) = f2 {
            cmd.stderr(f2);
        }
        if let Ok(d) = std::env::current_dir() {
            cmd.current_dir(d);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return out.err(&format!("cannot start {}: {}", native, e)),
        };
        let pid = child.id();
        let gone = out.gone();
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_millis() < warm as u128 && !gone() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if let Ok(Some(st)) = child.try_wait() {
                return out.err(&format!("the native build exited during warm-up ({}); log {}", st, log.display()));
            }
        }
        // its threads now
        let tids: Vec<i32> = std::fs::read_dir(format!("/proc/{}/task", pid)).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok())).collect()).unwrap_or_default();
        let rings: Vec<Ring> = tids.iter().filter_map(|&t| open(t, ev, hz)).collect();
        if rings.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return out.err(&format!("cannot sample the native process (perf_event_open on pid {} failed: perf_event_paranoid, or {} needs a PMU)", pid, ev.name()));
        }
        // both sampled over the same window: the native child on a thread here, this process by prof
        let (tx, rx) = std::sync::mpsc::channel::<Vec<(u64, u64)>>();
        let stop_at = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
        let spawned = {
            let _k = crate::zone_alloc::kept();
            std::thread::Builder::new().name("run-inspect-vsnative".into()).spawn(move || {
                crate::inspect::heap::exclude_this_thread();
                let mut v = Vec::new();
                while std::time::Instant::now() < stop_at {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    for r in &rings {
                        drain(r, &mut v);
                    }
                }
                for r in &rings {
                    drain(r, &mut v);
                    unsafe {
                        munmap(r.base as *mut u8, (1 + RING_PAGES) * PAGE);
                        close(r.fd);
                    }
                }
                let _ = tx.send(v);
            })
        };
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return out.err("cannot start the sampling thread");
        }
        let run = crate::inspect::prof::collect_ev(secs, hz, ev, &|t| !t.name.starts_with("run-"), &*gone);
        let nat = rx.recv_timeout(std::time::Duration::from_secs_f64(secs + 10.0)).unwrap_or_default();
        let map = Map::of(pid, &native);
        let _ = child.kill();
        let _ = child.wait();
        // native: per fn
        let mut nat_fn: HashMap<String, u64> = HashMap::new();
        let mut nat_total = 0u64;
        let mut unnamed = 0u64;
        let named = crate::inspect::asm::with_native_bin(&native, |nb| {
            for &(pc, w) in &nat {
                nat_total += w;
                match map.vaddr(pc).and_then(|va| nb.fn_at(va)) {
                    Some(f) => *nat_fn.entry(crate::inspect::asm_plain(&f.name)).or_default() += w,
                    None => unnamed += w,
                }
            }
        });
        if let Err(e) = named {
            return out.err(&e);
        }
        // ours: leaf frame per fn (JIT slots by name, host code by symbol)
        let mut partial = Vec::new();
        let leaves: Vec<Vec<u64>> = run.samples.iter().map(|s| s.frames.first().map(|&p| vec![p]).unwrap_or_default()).collect();
        let syms = crate::inspect::symbolise_within(&leaves, &mut partial, 3000);
        let mut jit_fn: HashMap<String, (u64, String)> = HashMap::new();
        let mut jit_total = 0u64;
        for (s, f) in run.samples.iter().zip(&syms) {
            let w = s.weight.max(1);
            jit_total += w;
            if let Some(f) = f.first() {
                let e = jit_fn.entry(crate::inspect::asm_plain(&f.name)).or_insert((0, f.module.clone()));
                e.0 += w;
            }
        }
        let per_s = |w: u64, t: f64| w as f64 / t.max(0.01);
        let (tj, tn) = (run.secs, secs);
        let mut keys: Vec<String> = jit_fn.keys().chain(nat_fn.keys()).cloned().collect();
        keys.sort();
        keys.dedup();
        let mut rows: Vec<(f64, String)> = keys
            .iter()
            .map(|k| {
                let (j, m) = jit_fn.get(k).cloned().unwrap_or((0, String::new()));
                let nv = nat_fn.get(k).copied().unwrap_or(0);
                let (jr, nr) = (per_s(j, tj), per_s(nv, tn));
                let ratio = if nr > 0.0 { format!("{:.2}", jr / nr) } else { "-".into() };
                (
                    jr - nr,
                    format!(
                        "| {} | {} | {} | {} | {} | {} | {} |",
                        short(&k.replace('|', "/"), 70),
                        g(jr),
                        g(nr),
                        ratio,
                        pct(j, jit_total),
                        pct(nv, nat_total),
                        if m.starts_with("jit") { m.split_whitespace().next().unwrap_or("").trim_start_matches("jit/").to_string() } else if j > 0 { "host".into() } else { String::new() }
                    ),
                )
            })
            .collect();
        rows.sort_by(|a, b| b.0.abs().partial_cmp(&a.0.abs()).unwrap_or(std::cmp::Ordering::Equal));
        let unit = match ev {
            crate::inspect::prof::Ev::Cpu => "CPU samples",
            e => e.name(),
        };
        out.always(&format!(
            "JIT {} {}/s vs native {}/s over {:.1} s ({:.2}x); biggest difference: {}",
            unit,
            g(per_s(jit_total, tj)),
            g(per_s(nat_total, tn)),
            secs,
            per_s(jit_total, tj) / per_s(nat_total, tn).max(1e-9),
            rows.first().map_or("-".to_string(), |r| r.1.split('|').nth(1).unwrap_or("").trim().to_string())
        ));
        out.line("");
        out.line(&format!("| fn | JIT {}/s | native {}/s | ratio | JIT % | native % | JIT tier |", unit, unit));
        out.line("|---|---:|---:|---:|---:|---:|---|");
        for (_, r) in rows.iter().take(n) {
            out.line(r);
        }
        if rows.len() > n {
            out.line(&format!("+{} fns with smaller differences", rows.len() - n));
        }
        out.always(&format!("note: native = {} {} (pid {}, warmed {} ms, log {}); {} of its samples outside its own code (libc, vdso: not named)", native, args.join(" "), pid, warm, log.display(), pct(unnamed, nat_total)));
        out.always("note: both at once on this machine, same event and rate; per second of wall fits fixed-pace work -- flat-out loops: compare the % columns (shares)");
        for p in partial.iter().chain(run.notes.iter()) {
            out.always(&format!("note: {}", p));
        }
        out.always(&format!("note: {}", crate::inspect::reliability::line(true)));
        out.always("more: asm <fn> vs=rustc  asm top vs=rustc  counters");
    }

    fn g(x: f64) -> String {
        if x >= 1e9 {
            format!("{:.2}G", x / 1e9)
        } else if x >= 1e6 {
            format!("{:.1}M", x / 1e6)
        } else if x >= 1e3 {
            format!("{:.1}k", x / 1e3)
        } else {
            format!("{:.0}", x)
        }
    }

    fn pct(a: u64, t: u64) -> String {
        if t == 0 {
            return "-".into();
        }
        format!("{:.1}", a as f64 * 100.0 / t as f64)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{Out, Req};
    pub fn run(_req: &Req, out: &mut Out) {
        out.err("vsnative samples a second process: Linux only (macOS allows that only with privileges); run it on a Linux node, or compare statically with `asm top vs=rustc`");
    }
}

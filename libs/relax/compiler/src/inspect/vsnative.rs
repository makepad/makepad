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
//! Linux only: macOS lets a process sample another one only with privileges. There (and anywhere)
//! `vsnative file=<raw>` compares against the native app's own samples (its `prof raw=<file>`).

use super::{Out, Req};

pub fn cmd_vsnative(req: &Req, out: &mut Out) {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(f) = req.get("file") {
        return stacks::run_file(req, out, f);
    }
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
    pub(super) struct Map {
        maps: Vec<(u64, u64, u64)>,
        loads: Vec<(u64, u64, u64)>,
    }

    impl Map {
        pub(super) fn of(pid: u32, exe: &str) -> Map {
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
        pub(super) fn vaddr(&self, pc: u64) -> Option<u64> {
            let &(a, _, o) = self.maps.iter().find(|m| pc >= m.0 && pc < m.1)?;
            let foff = pc - a + o;
            let &(po, pv, _) = self.loads.iter().find(|l| foff >= l.0 && foff < l.0 + l.2)?;
            Some(foff - po + pv)
        }
    }

    pub fn run(req: &Req, out: &mut Out) {
        if let Some(pid) = req.get("pid") {
            #[cfg(target_arch = "x86_64")]
            return super::stacks::run(req, out, pid);
            #[cfg(not(target_arch = "x86_64"))]
            return out.err(&format!("vsnative pid={}: another process's stacks are walked on x86_64 Linux only", pid));
        }
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

    pub(super) fn g(x: f64) -> String {
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

/// `vsnative pid=<native pid> [s=10] [hz=1000] [ev=cpu] [threads=main] [nthreads=<name>|busy] [roots=a,b]
/// [n=30] [tree=0] [min=0.1] [save=1]`: an already-running native build of the program (the same
/// scene, the same pace), sampled WITH STACKS over the same window as this process: both sides'
/// stacks expanded to source fns (the JIT's inline levels; the native build's DWARF inline tree)
/// and keyed alike (`Type::fn` without generics), so self and inclusive time per fn compare 1:1 --
/// the attribution table of a JIT-vs-native gap. `roots=` keeps the samples under those fns (the
/// part of a frame a meter times) and cuts their stacks at the root.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod stacks {
    use super::{Out, Req};
    use crate::inspect::short;
    use std::collections::HashMap;

    /// The key both sides share: the last two path segments of the plain name (no generics, no
    /// `<T as Trait>` qualifiers beyond T, no hash), a closure as `fn::{closure}`.
    fn key(name: &str) -> String {
        let n = name.replace("{{closure}}", "{closure}");
        let n = match n.strip_prefix("{closure} in ") {
            Some(rest) => format!("{}::{{closure}}", rest.split('<').next().unwrap_or(rest)),
            None => n,
        };
        let n = n.split(" (inlined").next().unwrap_or(&n).to_string();
        let n = n.trim_end_matches(" [unverified]").to_string();
        let p = crate::inspect::asm_plain(&n);
        let p = p.replace("::{closure}::{closure}", "::{closure}");
        let segs: Vec<&str> = p.split("::").filter(|x| !x.is_empty()).collect();
        let k = segs.len().saturating_sub(if segs.last() == Some(&"{closure}") { 3 } else { 2 });
        segs[k..].join("::")
    }

    /// This process's samples as expanded stacks, outermost first: (name, code) where code is the
    /// tier and build version of JIT code (`O3 v2`), `inl` for an inlined level, else the frame kind.
    fn jit_frames(run: &crate::inspect::prof::Run, partial: &mut Vec<String>) -> Vec<(Vec<(String, String)>, u64)> {
        crate::inspect::calltree::expanded_raw(run, partial).into_iter().map(|(_, _, f, w)| (f.into_iter().map(|x| {
            let jit = !matches!(x.tier.as_str(), "inl" | "native" | "glue" | "jit" | "?");
            let code = if jit { format!("{} v{}", x.tier, x.gen) } else { x.tier };
            (x.name, code)
        }).collect(), w)).collect()
    }

    /// The JIT-vs-native tables of `ours` (this process's run, expanded) and `theirs` (the native
    /// build's stacks over `tn` s, named by its DWARF inline tree), keyed alike, cut at `roots=`.
    /// Returns (unit, native all, native under roots, JIT all, JIT under roots, JIT secs).
    fn compare(req: &Req, out: &mut Out, run: &crate::inspect::prof::Run, ours: &[(Vec<(String, String)>, u64)], theirs: &[(Vec<(String, String)>, u64)], tn: f64) -> (String, f64, f64, f64, f64, f64) {
        let n = req.num("n", 30) as usize;
        let depth = req.num("tree", 0) as usize;
        let min = req.get("min").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.1);
        let ev = run.ev;
        let hz = run.hz;
        let secs = tn;
        let roots: Vec<String> = req.get("roots").map(|r| r.split(',').map(key).collect()).unwrap_or_default();
        // keyed stacks under the roots (cut at the outermost root), weights in ms of CPU (cpu) or events
        let unit_ms = if ev == crate::inspect::prof::Ev::Cpu { 1000.0 / hz as f64 } else { 1.0 };
        // per leaf key: the physical code it ran in (itself, or the fn it is inlined into) with that code's tier
        let mut owners: HashMap<String, HashMap<String, f64>> = HashMap::new();
        let mut keyed = |v: &[(Vec<(String, String)>, u64)], tiers: &mut HashMap<String, HashMap<String, u64>>, own: bool| -> (Vec<(Vec<String>, f64)>, f64) {
            let mut outv = Vec::new();
            let mut all = 0.0;
            for (fr, w) in v {
                let w = *w as f64 * unit_ms;
                all += w;
                let ks: Vec<String> = fr.iter().map(|f| key(&f.0)).collect();
                let at = if roots.is_empty() { Some(0) } else { ks.iter().position(|k| roots.contains(k)) };
                let Some(at) = at else { continue };
                for (k, f) in ks.iter().zip(fr.iter()).skip(at) {
                    if f.1 != "inl" {
                        *tiers.entry(k.clone()).or_default().entry(f.1.clone()).or_default() += w as u64;
                    }
                }
                if own {
                    let leaf = ks.len() - 1;
                    let phys = (at..=leaf).rev().find(|&p| fr[p].1 != "inl").unwrap_or(at);
                    let desc = if phys == leaf { format!("own code {}", fr[phys].1) } else { format!("inl in {} [{}]", ks[phys], fr[phys].1) };
                    *owners.entry(ks[leaf].clone()).or_default().entry(desc).or_default() += w;
                }
                outv.push((ks[at..].to_vec(), w));
            }
            (outv, all)
        };
        let mut jt: HashMap<String, HashMap<String, u64>> = HashMap::new();
        let mut nt = HashMap::new();
        let (j, jall) = keyed(ours, &mut jt, true);
        let (nv, nall) = keyed(theirs, &mut nt, false);
        let (tj, tn) = (run.secs.max(0.01), secs);
        let tables = |v: &[(Vec<String>, f64)]| {
            let mut selfm: HashMap<String, f64> = HashMap::new();
            let mut incl: HashMap<String, f64> = HashMap::new();
            let mut tot = 0.0;
            for (ks, w) in v {
                tot += w;
                if let Some(l) = ks.last() {
                    *selfm.entry(l.clone()).or_default() += w;
                }
                let mut seen: Vec<&String> = Vec::new();
                for k in ks {
                    if !seen.contains(&k) {
                        *incl.entry(k.clone()).or_default() += w;
                        seen.push(k);
                    }
                }
            }
            (selfm, incl, tot)
        };
        let (js, ji, jtot) = tables(&j);
        let (ns, ni, ntot) = tables(&nv);
        let (jr, nr) = (jtot / tj, ntot / tn);
        let unit = if ev == crate::inspect::prof::Ev::Cpu { "ms/s".to_string() } else { format!("{}/s", ev.name()) };
        let gap = jr - nr;
        let tier_of = |k: &str| jt.get(k).map(|m| {
            let mut v: Vec<(&String, &u64)> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            let t: u64 = v.iter().map(|x| *x.1).sum();
            v.iter().take(2).map(|(n, c)| if **c == t { n.to_string() } else { format!("{} {}%", n, **c * 100 / t.max(1)) }).collect::<Vec<_>>().join(" ")
        }).unwrap_or_else(|| "-".into());
        let mut srows: Vec<(f64, String)> = js.keys().chain(ns.keys()).map(|k| (js.get(k).copied().unwrap_or(0.0) / tj - ns.get(k).copied().unwrap_or(0.0) / tn, k.clone())).collect();
        srows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        srows.dedup_by(|a, b| a.1 == b.1);
        out.always(&format!(
            "JIT {:.1} {} vs native {:.1} {} under {} ({:.2}x, gap {:+.1} {}); biggest self-time gap: {}",
            jr, unit, nr, unit, if roots.is_empty() { "all stacks".to_string() } else { req.get("roots").unwrap_or("").to_string() }, jr / nr.max(1e-9), gap, unit,
            srows.first().map_or("-".to_string(), |r| format!("{} {:+.2}", r.1, r.0))
        ));
        out.line("");
        out.line(&format!("self time per fn ({}), sorted by JIT - native; cum = running share of the gap", unit));
        out.line("| fn | JIT tier | JIT code it ran in | JIT | native | delta | cum % |");
        out.line("|---|---|---|---:|---:|---:|---:|");
        let owner_of = |k: &str| owners.get(k).map(|m| {
            let mut v: Vec<(&String, &f64)> = m.iter().collect();
            v.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(b.0)));
            let t: f64 = v.iter().map(|x| *x.1).sum();
            v.iter().take(2).map(|(d, w)| if **w == t { d.to_string() } else { format!("{} {:.0}%", d, **w * 100.0 / t.max(1e-9)) }).collect::<Vec<_>>().join("; ")
        }).unwrap_or_else(|| "-".into());
        let mut cum = 0.0;
        for (d, k) in srows.iter().take(n) {
            cum += d;
            out.line(&format!("| {} | {} | {} | {:.2} | {:.2} | {:+.2} | {:.0} |", short(&k.replace('|', "/"), 70), tier_of(k), short(&owner_of(k).replace('|', "/"), 90), js.get(k).copied().unwrap_or(0.0) / tj, ns.get(k).copied().unwrap_or(0.0) / tn, d, cum * 100.0 / gap.abs().max(1e-9)));
        }
        let neg: Vec<String> = srows.iter().rev().take(6).filter(|r| r.0 < 0.0).map(|r| format!("{} {:+.2}", r.1, r.0)).collect();
        if !neg.is_empty() {
            out.line(&format!("native slower: {}", neg.join(", ")));
        }
        let mut irows: Vec<(f64, String)> = ji.keys().chain(ni.keys()).map(|k| (ji.get(k).copied().unwrap_or(0.0) / tj - ni.get(k).copied().unwrap_or(0.0) / tn, k.clone())).collect();
        irows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        irows.dedup_by(|a, b| a.1 == b.1);
        out.line("");
        out.line(&format!("inclusive time per fn ({}): the call trees the gap sits in", unit));
        out.line("| fn | JIT tier | JIT | native | delta |");
        out.line("|---|---|---:|---:|---:|");
        for (d, k) in irows.iter().take(n) {
            out.line(&format!("| {} | {} | {:.2} | {:.2} | {:+.2} |", short(&k.replace('|', "/"), 70), tier_of(k), ji.get(k).copied().unwrap_or(0.0) / tj, ni.get(k).copied().unwrap_or(0.0) / tn, d));
        }
        if depth > 0 {
            // merged top-down tree: (JIT, native) per path
            let mut tree: HashMap<Vec<String>, (f64, f64)> = HashMap::new();
            for (ks, w) in &j {
                for d in 1..=ks.len().min(depth) {
                    tree.entry(ks[..d].to_vec()).or_default().0 += w / tj;
                }
            }
            for (ks, w) in &nv {
                for d in 1..=ks.len().min(depth) {
                    tree.entry(ks[..d].to_vec()).or_default().1 += w / tn;
                }
            }
            out.line("");
            out.line(&format!("merged call tree to depth {} (JIT | native | delta, {}; rows with |delta| >= {})", depth, unit, min));
            fn walk(tree: &HashMap<Vec<String>, (f64, f64)>, path: &mut Vec<String>, depth: usize, min: f64, out: &mut Out) {
                let mut kids: Vec<(&Vec<String>, &(f64, f64))> = tree.iter().filter(|(p, _)| p.len() == path.len() + 1 && p[..path.len()] == path[..]).collect();
                kids.sort_by(|a, b| (b.1 .0 - b.1 .1).abs().partial_cmp(&(a.1 .0 - a.1 .1).abs()).unwrap_or(std::cmp::Ordering::Equal));
                for (p, v) in kids {
                    if (v.0 - v.1).abs() < min {
                        continue;
                    }
                    out.line(&format!("{}{}  {:.2} | {:.2} | {:+.2}", "  ".repeat(path.len()), short(p.last().unwrap(), 90), v.0, v.1, v.0 - v.1));
                    if p.len() < depth {
                        path.push(p.last().unwrap().clone());
                        walk(tree, path, depth, min, out);
                        path.pop();
                    }
                }
            }
            walk(&tree, &mut Vec::new(), depth, min, out);
        }
        if req.get("save") == Some("1") {
            for (side, v, t) in [("jit", &j, tj), ("native", &nv, tn)] {
                let mut folded: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
                for (ks, w) in v.iter() {
                    *folded.entry(ks.iter().map(|k| k.replace(';', ":")).collect::<Vec<_>>().join(";")).or_default() += w;
                }
                let path = std::env::temp_dir().join(format!("makepad-vsnative-{}-{}-{}.folded", std::process::id(), side, crate::inspect::now_ms()));
                let text: String = folded.iter().map(|(k, w)| format!("{} {:.0}\n", k, w)).collect();
                if std::fs::write(&path, text).is_ok() {
                    out.always(&format!("saved {} stacks ({} ms of CPU over {:.1} s): {}", side, v.iter().map(|x| x.1).sum::<f64>() as u64, t, path.display()));
                }
            }
        }
        (unit, nall, ntot, jall, jtot, tj)
    }

    /// `vsnative file=<raw samples> [native=<bin>] [s=<the file's>] [hz=<the file's>] [threads=main] [roots=a,b]
    /// [n=30] [tree=0] [save=1]`: the native build's stacks recorded by its own sampler (`prof raw=<file>`
    /// of a rustc-built app: macOS has no cross-process sampling without privileges), named here by its
    /// DWARF inline tree (`native=`, else the file's executable; macOS: its dSYM), against this
    /// process's samples taken now for the same length. For deterministic work (Sandbox's SANDBOX_DET
    /// frames) the two windows need not be simultaneous.
    pub fn run_file(req: &Req, out: &mut Out, path: &str) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => return out.err(&format!("cannot read {}: {}", path, e)),
        };
        let (mut exe, mut fsecs, mut fhz) = (String::new(), 0.0f64, 1000u64);
        let mut raw: Vec<(u64, Vec<String>)> = Vec::new();
        for l in text.lines() {
            let mut f = l.split('\t');
            match f.next() {
                Some("exe") => exe = f.next().unwrap_or("").to_string(),
                Some("secs") => fsecs = f.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
                Some("hz") => fhz = f.next().and_then(|v| v.parse().ok()).unwrap_or(1000),
                Some("s") => {
                    let w = f.next().and_then(|v| v.parse().ok()).unwrap_or(1);
                    raw.push((w, f.map(|x| x.to_string()).collect()));
                }
                _ => {}
            }
        }
        if raw.is_empty() || fsecs <= 0.0 {
            return out.err(&format!("{}: no samples (write it with `prof threads=main raw=<file>` of the native app)", path));
        }
        let native = req.get("native").map(|s| s.to_string()).unwrap_or(exe.clone());
        let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(fsecs).clamp(0.5, 60.0);
        let hz = req.num("hz", fhz).clamp(50, 4000);
        if hz != fhz {
            return out.err(&format!("hz={} differs from the native samples' {} Hz (CPU-time weights must match)", hz, fhz));
        }
        let ev = match crate::inspect::prof::Ev::parse(req.get("ev").unwrap_or("cpu")) {
            Ok(e) => e,
            Err(e) => return out.err(&e),
        };
        let gone = out.gone();
        let run = crate::inspect::prof::collect_ev(secs, hz, ev, &*crate::inspect::prof::pick(req.get("threads").unwrap_or("main")), &*gone);
        if gone() {
            return;
        }
        let mut partial = Vec::new();
        let ours: Vec<(Vec<(String, String)>, u64)> = jit_frames(&run, &mut partial);
        // native frames, innermost first in the file: `@<file vaddr>` in the executable, `!<name>` elsewhere
        let mut theirs: Vec<(Vec<(String, String)>, u64)> = Vec::new();
        let named = crate::inspect::asm::with_native_bin(&native, |nb| {
            let mut names: HashMap<u64, Vec<String>> = HashMap::new();
            let mut inl: HashMap<u64, Vec<crate::inspect::natbin::InlinedCall>> = HashMap::new();
            for (w, frames) in &raw {
                let mut fr: Vec<(String, String)> = Vec::new();
                for (i, x) in frames.iter().enumerate().rev() {
                    let Some(hex) = x.strip_prefix('@') else {
                        fr.push((x.strip_prefix('!').unwrap_or(x).to_string(), "native".to_string()));
                        continue;
                    };
                    let Ok(va) = u64::from_str_radix(hex, 16) else { continue };
                    // a return address is looked up one byte back, inside its call
                    let look = if i == 0 { va } else { va.wrapping_sub(1) };
                    let v = names.entry(look).or_insert_with(|| {
                        let Some(f) = nb.fn_at(look) else { return vec![format!("{}+{:#x}", native.rsplit('/').next().unwrap_or(&native), look)] };
                        let calls = inl.entry(f.addr).or_insert_with(|| nb.inlined(f.addr, f.addr + f.size));
                        let mut here: Vec<&crate::inspect::natbin::InlinedCall> = calls.iter().filter(|c| c.ranges.iter().any(|r| look >= r.0 && look < r.1)).collect();
                        here.sort_by_key(|c| std::cmp::Reverse(c.depth));
                        let mut v: Vec<String> = here.iter().map(|c| c.name.clone()).collect();
                        v.push(f.name.clone());
                        v
                    });
                    fr.extend(v.iter().rev().enumerate().map(|(k, n)| (n.clone(), if k == 0 { "native".to_string() } else { "inl".to_string() })));
                }
                theirs.push((fr, (*w).max(1)));
            }
        });
        if let Err(e) = named {
            return out.err(&e);
        }
        let (unit, nall, ntot, jall, jtot, tj) = compare(req, out, &run, &ours, &theirs, fsecs);
        out.always(&format!("note: native = {} (samples {}: {} over {:.1} s at {} Hz), {:.1} {} of its sampled CPU, {:.0}% under the roots; JIT {:.1} {} on threads={}, {:.0}% under the roots (sampled now, {:.1} s)", native, path, raw.len(), fsecs, fhz, nall / fsecs, unit, ntot * 100.0 / nall.max(1e-9), jall / tj, unit, req.get("threads").unwrap_or("main"), jtot * 100.0 / jall.max(1e-9), tj));
        out.always("note: keys are `Type::fn` without generics on both sides (JIT: inline levels expanded; native: the DWARF inline tree, needs debuginfo=2 / a dSYM); the two windows are not simultaneous: compare deterministic work only");
        for p in partial.iter().chain(run.notes.iter()) {
            out.always(&format!("note: {}", p));
        }
        out.always(&format!("note: {}", crate::inspect::reliability::line(true)));
    }

    /// (user ms, system ms, voluntary switches, involuntary switches) of thread `tid` of `pid` so far.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn tstat(pid: u32, tid: u64) -> (f64, f64, u64, u64) {
        let tick_ms = 10.0; // USER_HZ = 100 on Linux
        let st = std::fs::read_to_string(format!("/proc/{}/task/{}/stat", pid, tid)).unwrap_or_default();
        let f: Vec<&str> = st.rsplit_once(')').map(|x| x.1.split_whitespace().collect()).unwrap_or_default();
        let num = |k: usize| f.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let status = std::fs::read_to_string(format!("/proc/{}/task/{}/status", pid, tid)).unwrap_or_default();
        let sw = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
        (num(11) * tick_ms, num(12) * tick_ms, sw("voluntary_ctxt_switches:"), sw("nonvoluntary_ctxt_switches:"))
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn busiest(pid: u32, n: usize) -> Vec<(u64, String)> {
        let cpu = |t: &str| -> u64 {
            std::fs::read_to_string(format!("/proc/{}/task/{}/stat", pid, t)).ok().and_then(|s| {
                let f: Vec<&str> = s.rsplit_once(')')?.1.split_whitespace().collect();
                Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
            }).unwrap_or(0)
        };
        let tids: Vec<String> = std::fs::read_dir(format!("/proc/{}/task", pid)).map(|d| d.flatten().filter_map(|e| e.file_name().to_str().map(|s| s.to_string())).collect()).unwrap_or_default();
        let a: Vec<u64> = tids.iter().map(|t| cpu(t)).collect();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut v: Vec<(u64, u64, String)> = tids.iter().zip(&a).map(|(t, &c0)| {
            let name = std::fs::read_to_string(format!("/proc/{}/task/{}/comm", pid, t)).unwrap_or_default().trim().to_string();
            (cpu(t).saturating_sub(c0), t.parse().unwrap_or(0), name)
        }).collect();
        v.sort_by(|x, y| y.0.cmp(&x.0));
        v.into_iter().take(n).map(|x| (x.1, x.2)).collect()
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub fn run(req: &Req, out: &mut Out, pid: &str) {
        let Ok(pid) = pid.parse::<u32>() else { return out.err("pid=<the native build's process id>") };
        let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(10.0).clamp(0.5, 60.0);
        let hz = req.num("hz", 1000).clamp(50, 4000);
        let ev = match crate::inspect::prof::Ev::parse(req.get("ev").unwrap_or("cpu")) {
            Ok(e) => e,
            Err(e) => return out.err(&e),
        };
        let exe = match std::fs::read_link(format!("/proc/{}/exe", pid)) {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => return out.err(&format!("no process {}: {}", pid, e)),
        };
        let cfi = match crate::inspect::cfi::Foreign::open(pid) {
            Ok(c) => c,
            Err(e) => return out.err(&e),
        };
        // the native threads: by name, else the busiest
        let tids: Vec<(u64, String)> = match req.get("nthreads") {
            Some(sel) if sel != "busy" => busiest(pid, 1 << 16).into_iter().filter(|(_, nm)| sel.split(',').any(|s| nm.starts_with(s))).collect(),
            _ => busiest(pid, 2),
        };
        if tids.is_empty() {
            return out.err(&format!("process {}: no thread matches nthreads=", pid));
        }
        let gone = out.gone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ids: Vec<u64> = tids.iter().map(|t| t.0).collect();
        let spawned = {
            let _k = crate::zone_alloc::kept();
            std::thread::Builder::new().name("run-inspect-vsnative".into()).spawn(move || {
                crate::inspect::heap::exclude_this_thread();
                let r = crate::inspect::prof::collect_foreign(&ids, secs, hz, ev, &cfi, &|| false);
                let _ = tx.send((r, cfi));
            })
        };
        if spawned.is_err() {
            return out.err("cannot start the sampling thread");
        }
        // user/system time and context switches of both sides' threads over the window: the
        // samples see user time only (perf_event_paranoid 2), a meter of wall time sees all of it
        let me = std::process::id();
        let mine: Vec<u64> = {
            let sel = crate::inspect::prof::pick(req.get("threads").unwrap_or("main"));
            let mut ts = crate::inspect::threads::list();
            crate::inspect::name_handles(&mut ts);
            ts.into_iter().filter(|t| sel(t)).map(|t| t.os_id).collect()
        };
        let st0: Vec<(f64, f64, u64, u64)> = mine.iter().map(|&t| tstat(me, t)).chain(tids.iter().map(|t| tstat(pid, t.0))).collect();
        let run = crate::inspect::prof::collect_ev(secs, hz, ev, &*crate::inspect::prof::pick(req.get("threads").unwrap_or("main")), &*gone);
        let st1: Vec<(f64, f64, u64, u64)> = mine.iter().map(|&t| tstat(me, t)).chain(tids.iter().map(|t| tstat(pid, t.0))).collect();
        let Ok(((nat, lost, mut notes), cfi)) = rx.recv_timeout(std::time::Duration::from_secs_f64(secs + 30.0)) else {
            return out.err("the native sampling thread did not finish");
        };
        if gone() {
            return;
        }
        // ours: expanded (inline levels as frames), outermost first
        let mut partial = Vec::new();
        let ours: Vec<(Vec<(String, String)>, u64)> = jit_frames(&run, &mut partial);
        // native: each pc by the binary's symbols and inline tree
        let map = super::imp::Map::of(pid, &exe);
        let mut names: HashMap<u64, Vec<String>> = HashMap::new();
        let mut theirs: Vec<(Vec<(String, String)>, u64)> = Vec::new();
        let named = crate::inspect::asm::with_native_bin(&exe, |nb| {
            let mut inl: HashMap<u64, Vec<crate::inspect::natbin::InlinedCall>> = HashMap::new();
            for smp in &nat {
                let mut fr: Vec<(String, String)> = Vec::new();
                for (i, &pc) in smp.frames.iter().enumerate().rev() {
                    let look = if i == 0 { pc } else { pc.wrapping_sub(1) };
                    let v = names.entry(look).or_insert_with(|| {
                        let Some(va) = map.vaddr(look) else {
                            return vec![cfi.files.iter().find(|f| look >= f.0 && look < f.1).map_or_else(|| format!("{:#x}", look), |f| format!("{}+{:#x}", f.2.rsplit('/').next().unwrap_or(&f.2), look - f.0))];
                        };
                        let Some(f) = nb.fn_at(va) else { return vec![format!("{}+{:#x}", exe.rsplit('/').next().unwrap_or(&exe), va)] };
                        let calls = inl.entry(f.addr).or_insert_with(|| nb.inlined(f.addr, f.addr + f.size));
                        let mut here: Vec<&crate::inspect::natbin::InlinedCall> = calls.iter().filter(|c| c.ranges.iter().any(|r| va >= r.0 && va < r.1)).collect();
                        here.sort_by_key(|c| std::cmp::Reverse(c.depth));
                        // innermost first, the concrete fn last
                        let mut v: Vec<String> = here.iter().map(|c| c.name.clone()).collect();
                        v.push(f.name.clone());
                        v
                    });
                    // outermost first
                    fr.extend(v.iter().rev().enumerate().map(|(k, n)| (n.clone(), if k == 0 { "native".to_string() } else { "inl".to_string() })));
                }
                theirs.push((fr, smp.weight.max(1)));
            }
        });
        if let Err(e) = named {
            return out.err(&e);
        }
        let (unit, nall, ntot, jall, jtot, tj) = compare(req, out, &run, &ours, &theirs, secs);
        // per-thread OS time over the window, per second
        let rows: Vec<String> = mine.iter().map(|&t| ("JIT".to_string(), format!("T{}", t))).chain(tids.iter().map(|t| ("native".to_string(), format!("{} T{}", t.1, t.0)))).zip(st0.iter().zip(&st1)).map(|((side, name), (a, b))| {
            format!("| {} | {} | {:.1} | {:.1} | {:.0} | {:.0} |", side, name, (b.0 - a.0) / run.secs.max(0.01), (b.1 - a.1) / run.secs.max(0.01), (b.2 - a.2) as f64 / run.secs.max(0.01), (b.3 - a.3) as f64 / run.secs.max(0.01))
        }).collect();
        out.line("");
        out.line("OS time of the sampled threads over the window (whole thread, not cut at the roots; 10-ms ticks)");
        out.line("| side | thread | user ms/s | system ms/s | voluntary switches/s | involuntary/s |");
        out.line("|---|---|---:|---:|---:|---:|");
        for r in rows {
            out.line(&r);
        }
        let tn_names: Vec<String> = tids.iter().map(|t| format!("{} (T{})", t.1, t.0)).collect();
        out.always(&format!("note: native = {} pid {} threads {}; {:.1} {} of its CPU in the window, {:.0}% under the roots; JIT {:.1} {} on threads={}, {:.0}% under the roots", exe, pid, tn_names.join(", "), nall / secs, unit, ntot * 100.0 / nall.max(1e-9), jall / tj, unit, req.get("threads").unwrap_or("main"), jtot * 100.0 / jall.max(1e-9)));
        out.always("note: keys are `Type::fn` without generics on both sides (JIT: inline levels expanded; native: the DWARF inline tree, needs debuginfo=2); a fn rustc inlined and we did not shows as self time of its caller natively");
        if lost > 0 {
            notes.push(format!("{} native sample batches lost (ring overflow)", lost));
        }
        for p in partial.iter().chain(run.notes.iter()).chain(notes.iter()) {
            out.always(&format!("note: {}", p));
        }
        out.always(&format!("note: {}", crate::inspect::reliability::line(true)));
        out.always("more: asm <fn> vs=rustc  asm top vs=rustc  prof view=tree");
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{Out, Req};
    pub fn run(_req: &Req, out: &mut Out) {
        out.err("vsnative samples a second process: Linux only (macOS allows that only with privileges); on macOS give it the native app's own samples (`vsnative file=<the native app's prof raw= file>`), or compare statically with `asm top vs=rustc`");
    }
}

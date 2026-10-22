//! `counters [s=1] [threads=...]`: hardware counters over a window, as far as the OS gives them to
//! an unprivileged process without a prompt.
//! - Linux: perf_event counting per thread (cycles, instructions, branch misses, cache misses),
//!   scaled when the kernel multiplexed them.
//! - macOS: per-thread hardware counters need root; the process's own instructions and cycles
//!   come from proc_pid_rusage (rusage_info v4), summed over all its threads.
//! Per-fn and per-instruction event counts are sampling: `asm <fn> ev=cycles`, `asm top ev=cycles`.

use super::{Out, Req};

/// `counters [s=1] [n=12]`
pub fn cmd_counters(req: &Req, out: &mut Out) {
    let secs = req.get("s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0).clamp(0.1, 30.0);
    let n = req.num("n", 12) as usize;
    imp::run(secs, n, out);
    out.always(&format!("note: {}", super::reliability::line(true)));
    out.always("more: asm top vs=rustc ev=cycles  asm <fn> ev=instructions  prof threads=main");
}

fn g(x: f64) -> String {
    if x >= 1e9 {
        format!("{:.2} G", x / 1e9)
    } else if x >= 1e6 {
        format!("{:.1} M", x / 1e6)
    } else if x >= 1e3 {
        format!("{:.0} k", x / 1e3)
    } else {
        format!("{:.0}", x)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{g, Out};
    use crate::inspect::short;

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn close(fd: i32) -> i32;
    }

    #[cfg(target_arch = "x86_64")]
    const SYS_PERF_EVENT_OPEN: i64 = 298;
    #[cfg(target_arch = "aarch64")]
    const SYS_PERF_EVENT_OPEN: i64 = 241;

    /// A counting (not sampling) event of thread `tid`: user space only.
    fn open(tid: u64, config: u64) -> Option<i32> {
        let mut attr = [0u8; 128];
        let put = |a: &mut [u8; 128], off: usize, v: &[u8]| a[off..off + v.len()].copy_from_slice(v);
        put(&mut attr, 0, &0u32.to_ne_bytes()); // PERF_TYPE_HARDWARE
        put(&mut attr, 4, &128u32.to_ne_bytes());
        put(&mut attr, 8, &config.to_ne_bytes());
        put(&mut attr, 32, &(1u64 | 2).to_ne_bytes()); // read_format: TOTAL_TIME_ENABLED | RUNNING
        put(&mut attr, 40, &((1u64 << 5) | (1u64 << 6)).to_ne_bytes()); // exclude_kernel | exclude_hv
        let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), tid as i32, -1i32, -1i32, 0u64) } as i32;
        (fd >= 0).then_some(fd)
    }

    /// The count, scaled by enabled / running time (multiplexed counters).
    fn value(fd: i32) -> Option<f64> {
        let mut b = [0u64; 3];
        let r = unsafe { read(fd, b.as_mut_ptr() as *mut u8, 24) };
        if r != 24 || b[2] == 0 {
            return None;
        }
        Some(b[0] as f64 * b[1] as f64 / b[2] as f64)
    }

    const EVENTS: [(u64, &str); 4] = [(0, "cycles"), (1, "instructions"), (5, "branch misses"), (3, "cache misses")];

    pub fn run(secs: f64, n: usize, out: &mut Out) {
        let skip = crate::inspect::SERVER_TIDS.lock().unwrap().clone();
        let mut threads = crate::inspect::threads::list();
        crate::inspect::name_handles(&mut threads);
        threads.retain(|t| !crate::inspect::is_inspect_thread(t, &skip));
        let fds: Vec<(usize, Vec<Option<i32>>)> = threads.iter().enumerate().map(|(k, t)| (k, EVENTS.iter().map(|e| open(t.os_id, e.0)).collect())).collect();
        if fds.iter().all(|(_, v)| v.iter().all(|x| x.is_none())) {
            out.always("no hardware counters for this process (perf_event_paranoid > 2, or no PMU in this VM); CPU time only: prof, why-slow");
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        let mut rows: Vec<(f64, String)> = Vec::new();
        let mut tot = [0f64; 4];
        for (k, v) in &fds {
            let vals: Vec<Option<f64>> = v.iter().map(|fd| fd.and_then(value)).collect();
            for fd in v.iter().flatten() {
                unsafe { close(*fd) };
            }
            for (i, x) in vals.iter().enumerate() {
                tot[i] += x.unwrap_or(0.0);
            }
            let cyc = vals[0].unwrap_or(0.0);
            if cyc <= 0.0 {
                continue;
            }
            let ins = vals[1].unwrap_or(0.0);
            let t = &threads[*k];
            rows.push((
                cyc,
                format!(
                    "| {} (T{}) | {} | {} | {} | {} | {} |",
                    short(if t.name.is_empty() { "-" } else { &t.name }, 40),
                    t.os_id,
                    g(cyc),
                    g(ins),
                    if cyc > 0.0 { format!("{:.2}", ins / cyc) } else { "-".into() },
                    vals[2].map_or("-".into(), |b| format!("{} ({:.2}/k instr)", g(b), b * 1000.0 / ins.max(1.0))),
                    vals[3].map_or("-".into(), |c| format!("{} ({:.2}/k instr)", g(c), c * 1000.0 / ins.max(1.0))),
                ),
            ));
        }
        rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        out.always(&format!(
            "{:.1} s: {} cycles, {} instructions (IPC {:.2}) in user space over {} threads; busiest: {}",
            secs,
            g(tot[0]),
            g(tot[1]),
            tot[1] / tot[0].max(1.0),
            rows.len(),
            rows.first().map_or("-".to_string(), |r| r.1.split('|').nth(1).unwrap_or("").trim().to_string())
        ));
        out.line("");
        out.line("| thread | cycles | instructions | IPC | branch misses | cache misses |");
        out.line("|---|---:|---:|---:|---:|---:|");
        for (_, r) in rows.iter().take(n) {
            out.line(r);
        }
        if rows.len() > n {
            let rest: f64 = rows[n..].iter().map(|r| r.0).sum();
            out.line(&format!("+{} threads, {} cycles", rows.len() - n, g(rest)));
        }
        out.always("note: perf_event counting, user space only, per thread; scaled where the kernel multiplexed counters");
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{g, Out};

    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
    }

    /// rusage_info_v4: ri_instructions and ri_cycles are the 30th and 31st u64 after the 16-byte uuid.
    fn sample() -> Option<(u64, u64, u64)> {
        let mut b = [0u64; 64];
        if unsafe { proc_pid_rusage(std::process::id() as i32, 4, b.as_mut_ptr()) } != 0 {
            return None;
        }
        // 2 u64 of uuid, then user time [2], ..., instructions [31], cycles [32]
        Some((b[31], b[32], b[2] + b[3]))
    }

    pub fn run(secs: f64, _n: usize, out: &mut Out) {
        let Some(a) = sample() else {
            out.always("no counters: proc_pid_rusage failed");
            return;
        };
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        let Some(b) = sample() else {
            out.always("no counters: proc_pid_rusage failed");
            return;
        };
        let ins = b.0.saturating_sub(a.0) as f64;
        let cyc = b.1.saturating_sub(a.1) as f64;
        if cyc == 0.0 {
            out.always(&format!("{:.1} s: this macOS reports no instruction/cycle counts for the process (rusage v4 counters are 0)", secs));
            return;
        }
        out.always(&format!("{:.1} s: {} cycles, {} instructions (IPC {:.2}) for the whole process (all threads)", secs, g(cyc), g(ins), ins / cyc));
        out.always("note: macOS gives an unprivileged process only its whole-process instructions and cycles (proc_pid_rusage); per-thread counters, branch and cache misses need root: not shown. Per-fn costs: asm <fn> (CPU-time samples)");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use super::Out;
    pub fn run(_secs: f64, _n: usize, out: &mut Out) {
        out.always("no hardware counters on this target");
    }
}

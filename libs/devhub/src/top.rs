//! `top`: what a node is busy with right now, for an agent in one call: CPU use (all cores, over
//! a short sample), load, RAM, disk, each GPU's use, memory and temperature, and the busiest
//! processes (CPU over the same sample, RSS, GPU memory). Read from the OS (`/proc`,
//! `nvidia-smi`; macOS `ps`), nothing is asked of the processes.

use makepad_remote::json_str;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One GPU's live numbers (`nvidia-smi`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuUse {
    pub name: String,
    pub util_pct: f64,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub temp_c: f64,
    pub power_w: f64,
}

/// `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw
/// --format=csv,noheader,nounits` lines.
pub fn parse_gpu_use(text: &str) -> Vec<GpuUse> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            if f.len() < 6 || f[0].is_empty() {
                return None;
            }
            let n = |s: &str| s.parse::<f64>().unwrap_or(0.0);
            Some(GpuUse { name: f[0].to_string(), util_pct: n(f[1]), mem_used_gb: n(f[2]) / 1024.0, mem_total_gb: n(f[3]) / 1024.0, temp_c: n(f[4]), power_w: n(f[5]) })
        })
        .collect()
}

/// The driver's tool where it is installed (no PATH search on every call).
pub fn nvidia_smi() -> Option<&'static str> {
    ["/usr/bin/nvidia-smi", "/usr/local/bin/nvidia-smi", "/opt/bin/nvidia-smi", "C:\\Windows\\System32\\nvidia-smi.exe"].into_iter().find(|p| std::path::Path::new(p).exists())
}

fn smi(args: &[&str]) -> Option<String> {
    let o = makepad_remote::group::command(nvidia_smi()?).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// `ioreg -r -d 1 -w 0 -c <class>` on a Mac: the registry's entries of that class. None elsewhere.
fn ioreg(class: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let o = makepad_remote::group::command("/usr/sbin/ioreg").args(["-r", "-d", "1", "-w", "0", "-c", class]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// The GPU time of each process that holds a Mac's GPU, in `ioreg -r -d 1 -w 0 -c
/// IOGPUDeviceUserClient`: each client's creator (`"IOUserClientCreator" = "pid 487,
/// WindowServer"`) and the `"accumulatedGPUTime"` (ns) its `AppUsage` lists, summed by pid. A Mac
/// tells each process's GPU time so without root.
pub fn parse_gpu_times(text: &str) -> Vec<(u32, u64)> {
    let number = |rest: &str| rest[..rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len())].parse::<u64>().ok();
    let mut by_pid = std::collections::BTreeMap::<u32, u64>::new();
    for client in text.split("+-o ").skip(1) {
        let creator = "\"IOUserClientCreator\" = \"pid ";
        let Some(pid) = client.find(creator).and_then(|at| number(&client[at + creator.len()..])) else { continue };
        let key = "\"accumulatedGPUTime\"=";
        let ns: u64 = client.match_indices(key).filter_map(|(at, _)| number(&client[at + key.len()..])).sum();
        *by_pid.entry(pid as u32).or_default() += ns;
    }
    by_pid.into_iter().collect()
}

/// Each pid's GPU use between two looks at the clients' times ([`parse_gpu_times`]) `span`
/// apart, % of the GPU. A new pid's time is not all in the span; it and zero deltas are left out.
pub fn gpu_rates(before: &[(u32, u64)], after: &[(u32, u64)], span: Duration) -> Vec<(u32, f64)> {
    let span_ns = span.as_nanos().max(1) as f64;
    after.iter().filter_map(|(pid, ns)| {
        let was = before.iter().find(|(p, _)| p == pid)?.1;
        let pct = ns.saturating_sub(was) as f64 * 100.0 / span_ns;
        (pct > 0.0).then_some((*pid, pct))
    }).collect()
}

/// GPU rates placed by `rows` as [`outside_between`] places them (`whose` says `""` for own groups):
/// in all and by process, busiest first. The hub itself and processes gone from `rows` are left out.
pub fn gpu_split(rates: &[(u32, f64)], rows: &[ProcRow], me: u32, whose: &dyn Fn(u32) -> Option<String>) -> (f64, Vec<(String, u32, f64)>) {
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut v: Vec<(String, u32, f64)> = rates.iter().filter_map(|(pid, pct)| {
        let r = by_pid.get(pid).filter(|r| r.pid != me)?;
        match owner(&by_pid, r, me, whose) {
            Some(w) if w.is_empty() => None,
            Some(w) => Some((format!("{} ({w})", r.name), *pid, *pct)),
            None => Some((r.name.clone(), *pid, *pct)),
        }
    }).collect();
    v.sort_by(|a, b| b.2.total_cmp(&a.2));
    (v.iter().map(|(_, _, p)| p).sum(), v)
}

/// GPU rates since this caller's previous look. None off a Mac, on the first look, or if the
/// registry could not be read. Caps and timed verdicts keep independent sampling state.
pub(crate) fn gpu_since(last: &Mutex<Option<(Instant, Vec<(u32, u64)>)>>) -> Option<Vec<(u32, f64)>> {
    let times = parse_gpu_times(&ioreg("IOGPUDeviceUserClient")?);
    let now = Instant::now();
    let (then, before) = last.lock().unwrap().replace((now, times.clone()))?;
    Some(gpu_rates(&before, &times, now.duration_since(then)))
}

/// Per-process GPU use over a half-second look for a start check with no previous sample.
/// None off a Mac or if the registry could not be read.
pub fn gpu_look() -> Option<Vec<(u32, f64)>> {
    let before = parse_gpu_times(&ioreg("IOGPUDeviceUserClient")?);
    let then = Instant::now();
    std::thread::sleep(Duration::from_millis(500));
    let after = parse_gpu_times(&ioreg("IOGPUDeviceUserClient")?);
    Some(gpu_rates(&before, &after, then.elapsed()))
}

/// [`gpu_split`] since the previous call (a timed lease's box look, every 10 s) on a Mac. None
/// elsewhere, on the first call, or if the registry could not be read.
pub fn gpu_outside(rows: &[ProcRow], whose: &dyn Fn(u32) -> Option<String>) -> Option<(f64, Vec<(String, u32, f64)>)> {
    static LAST: Mutex<Option<(Instant, Vec<(u32, u64)>)>> = Mutex::new(None);
    Some(gpu_split(&gpu_since(&LAST)?, rows, std::process::id(), whose))
}

/// The busiest three users from [`gpu_split`], with their pid and percent of the GPU.
pub fn gpu_said(on: &[(String, u32, f64)]) -> String {
    on.iter().take(3).map(|(what, pid, pct)| format!("{what} pid {pid} at {pct:.0}% of the GPU")).collect::<Vec<_>>().join(", ")
}

/// Every GPU's use now (empty without an NVIDIA driver).
pub fn gpu_use() -> Vec<GpuUse> {
    smi(&["--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader,nounits"]).map_or_else(Vec::new, |t| parse_gpu_use(&t))
}

/// pid -> GPU memory in MB of the processes on the GPUs.
fn gpu_procs() -> Vec<(u32, f64)> {
    smi(&["--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (p, m) = l.split_once(',')?;
            Some((p.trim().parse().ok()?, m.trim().parse().ok()?))
        })
        .collect()
}

/// (busy, total) jiffies of the `cpu` line of `/proc/stat`.
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    // idle + iowait
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// CPU use in percent of all cores between two `/proc/stat` samples.
pub fn cpu_pct(a: (u64, u64), b: (u64, u64)) -> f64 {
    let (busy, total) = (b.0.saturating_sub(a.0), b.1.saturating_sub(a.1));
    if total == 0 { 0.0 } else { busy as f64 * 100.0 / total as f64 }
}

/// A process of the box, with its CPU over a look ([`processes`]).
#[derive(Clone, Debug, Default)]
pub struct ProcRow {
    pub pid: u32,
    /// its parent (Windows: unknown, 0)
    pub ppid: u32,
    /// its process group (Windows: itself)
    pub pgid: u32,
    pub name: String,
    pub cpu_pct: f64,
    pub rss_mb: f64,
    pub gpu_mb: f64,
}

/// Each process's `/proc/<pid>/stat`: its pid, name and the fields after the name (`state` first).
#[cfg(target_os = "linux")]
fn each_proc(mut each: impl FnMut(u32, String, &dyn Fn(usize) -> u64)) {
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
        let f: Vec<&str> = stat[b + 2..].split_whitespace().collect();
        each(pid, stat[a + 1..b].to_string(), &|i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0));
    }
}

/// Every process: pid -> (name, parent, group, CPU ticks so far, RSS bytes).
#[cfg(target_os = "linux")]
fn procs() -> std::collections::HashMap<u32, (String, u32, u32, u64, u64)> {
    let mut out = std::collections::HashMap::new();
    each_proc(|pid, name, num| {
        out.insert(pid, (name, num(1) as u32, num(2) as u32, num(11) + num(12), num(21) * 4096));
    });
    out
}

/// A process's CPU so far, at a timed lease's look ([`ticks`], [`outside_between`]).
#[cfg(not(windows))]
#[derive(Clone, Debug, Default)]
pub struct ProcTicks {
    pub name: String,
    pub ppid: u32,
    pub pgid: u32,
    /// when it started (Linux: its start in boot ticks; elsewhere 0): a pid seen again with
    /// another start is another process
    pub start: u64,
    /// its own CPU so far, in [`Ticks::per_s`] units
    pub own: u64,
    /// the CPU of the children it waited for, so far (Linux's cutime and cstime; 0 elsewhere)
    pub reaped: u64,
}

/// The box's CPU so far at a timed lease's look: each process's, and on Linux the box's busy time
/// and its interrupts' (`/proc/stat`, over all cores).
#[cfg(not(windows))]
#[derive(Clone, Debug)]
pub struct Ticks {
    pub at: Instant,
    /// the units of its counts per second
    pub per_s: f64,
    pub procs: std::collections::HashMap<u32, ProcTicks>,
    /// (busy, interrupts) so far (Linux)
    pub cpu: Option<(u64, u64)>,
}

/// The box's [`Ticks`] now: Linux reads `/proc` in place; elsewhere `ps`'s CPU time of each
/// process (one spawn: libproc refuses other users' processes to this one).
#[cfg(target_os = "linux")]
pub fn ticks() -> Ticks {
    let at = Instant::now();
    let cpu = std::fs::read_to_string("/proc/stat").ok().and_then(|t| parse_proc_busy_irq(&t));
    let mut procs = std::collections::HashMap::new();
    each_proc(|pid, name, num| {
        procs.insert(pid, ProcTicks { name, ppid: num(1) as u32, pgid: num(2) as u32, start: num(19), own: num(11) + num(12), reaped: num(13) + num(14) });
    });
    Ticks { at, per_s: ticks_per_s(), procs, cpu }
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn ticks() -> Ticks {
    let at = Instant::now();
    let out = makepad_remote::group::command("/bin/ps").args(["-Ao", "pid=,ppid=,pgid=,time=,comm="]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let procs = text
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let pid = w.next()?.parse().ok()?;
            let ppid = w.next()?.parse().ok()?;
            let pgid = w.next()?.parse().ok()?;
            let own = cpu_time_cs(w.next()?)?;
            let comm = w.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            Some((pid, ProcTicks { name, ppid, pgid, own, ..Default::default() }))
        })
        .collect();
    Ticks { at, per_s: 100.0, procs, cpu: None }
}

/// `ps`'s CPU time so far (`[dd-][hh:]mm:ss[.ss]`) in hundredths of a second.
#[cfg(any(not(any(target_os = "linux", windows)), test))]
fn cpu_time_cs(t: &str) -> Option<u64> {
    let (days, rest) = match t.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, t),
    };
    let mut secs = 0.0f64;
    for part in rest.split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(((days as f64 * 86_400.0 + secs) * 100.0).round() as u64)
}

/// (busy, interrupts) jiffies of `/proc/stat`'s `cpu` line ([`parse_proc_stat`]'s busy; irq and
/// softirq).
pub fn parse_proc_busy_irq(text: &str) -> Option<(u64, u64)> {
    let (busy, _) = parse_proc_stat(text)?;
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    Some((busy, v.get(5).copied().unwrap_or(0) + v.get(6).copied().unwrap_or(0)))
}

/// What ran outside a timed lease between two of its looks ([`outside_between`]), % of one core
/// over the span.
#[cfg(not(windows))]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outside {
    pub secs: f64,
    pub load1: f64,
    /// (what, its CPU with what the children it waited for took, the children's part): each
    /// program with CPU outside the lease's groups, the busiest first; the hub apart
    pub by: Vec<(String, f64, f64)>,
    /// the hub's own CPU
    pub hub: f64,
    /// Linux: (the box's CPU no process showed, interrupts' part of it)
    pub unseen: Option<(f64, f64)>,
}

/// What ran outside a timed lease between its looks `a` and `b`, exact: each process's CPU over
/// the span, by whose it is ([`owner`]: `whose(pgid)` names a lease's groups, "" the timed lease's
/// own; `me` the hub), each program outside it with what the children it waited for took since
/// (they came and went between the looks: a GPU box's AI hub ran `nvidia-smi` about twice a
/// second, 70 ms of CPU each, 13% of a core that the half-second samples of the looks before saw
/// almost none of, 11-22), the hub's own, and on Linux what no process showed (interrupts, and
/// processes no one waited for: orphans the init took, the hub's own children). A child's CPU
/// counts once: a parent's waited-for count holds again what its children that ended since had
/// taken by the first look, which their own counts carried. The hub's and an init's are not
/// counted (the hub's are its leases' runs, an init's anyone's orphans). Also the span's rows
/// (each process's own CPU), for the GPU split's names.
#[cfg(not(windows))]
pub fn outside_between(a: &Ticks, b: &Ticks, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> (Outside, Vec<ProcRow>) {
    use std::collections::{BTreeMap, HashMap};
    let secs = b.at.saturating_duration_since(a.at).as_secs_f64().max(0.001);
    let pct = |t: u64| t as f64 / b.per_s / secs * 100.0;
    // the process at the first look, if it is the same one
    let was = |pid: u32, q: &ProcTicks| a.procs.get(&pid).filter(|p| p.start == q.start && p.own <= q.own && p.reaped <= q.reaped);
    let mut gone: HashMap<u32, u64> = HashMap::new();
    for (pid, p) in &a.procs {
        if b.procs.get(pid).is_none_or(|q| was(*pid, q).is_none()) {
            *gone.entry(p.ppid).or_default() += p.own + p.reaped;
        }
    }
    let rows: Vec<ProcRow> = b.procs.iter().map(|(pid, q)| {
        let own = q.own - was(*pid, q).map_or(0, |p| p.own);
        ProcRow { pid: *pid, ppid: q.ppid, pgid: q.pgid, name: q.name.clone(), cpu_pct: pct(own), ..Default::default() }
    }).collect();
    let by_pid: HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let init = |r: &ProcRow| r.pid <= 1 || matches!(r.name.as_str(), "systemd" | "init" | "launchd");
    let (mut hub, mut seen) = (0.0, 0.0);
    let mut by: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for r in &rows {
        let q = &b.procs[&r.pid];
        let kids = if r.pid == me || init(r) { 0.0 } else { pct((q.reaped - was(r.pid, q).map_or(0, |p| p.reaped)).saturating_sub(gone.get(&r.pid).copied().unwrap_or(0))) };
        let cpu = r.cpu_pct + kids;
        if cpu <= 0.0 {
            continue;
        }
        seen += cpu;
        if r.pid == me {
            hub += cpu;
            continue;
        }
        let name = label(r.pid).unwrap_or_else(|| r.name.clone());
        let what = match owner(&by_pid, r, me, whose) {
            Some(w) if w.is_empty() => continue,
            Some(w) => format!("{name} ({w})"),
            None => name,
        };
        let e = by.entry(what).or_default();
        e.0 += cpu;
        e.1 += kids;
    }
    let mut by: Vec<(String, f64, f64)> = by.into_iter().map(|(what, (cpu, kids))| (what, cpu, kids)).collect();
    by.sort_by(|x, y| y.1.total_cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    let unseen = match (a.cpu, b.cpu) {
        (Some(x), Some(y)) => {
            let rest = (pct(y.0.saturating_sub(x.0)) - seen).max(0.0);
            Some((rest, pct(y.1.saturating_sub(x.1)).min(rest)))
        }
        _ => None,
    };
    (Outside { secs, load1: crate::procinfo::machine().load1, by, hub, unseen }, rows)
}

/// The kernel's CPU ticks per second.
#[cfg(target_os = "linux")]
pub(crate) fn ticks_per_s() -> f64 {
    extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    let t = unsafe { sysconf(2) };
    if t > 0 { t as f64 } else { 100.0 }
}

/// Each process's CPU (% of one core) from ticks `before` to `after` over `secs`; a process not
/// seen before started since: all its ticks are of the span.
#[cfg(target_os = "linux")]
fn rows_between(before: &std::collections::HashMap<u32, u64>, after: std::collections::HashMap<u32, (String, u32, u32, u64, u64)>, secs: f64) -> Vec<ProcRow> {
    let tck = ticks_per_s();
    after
        .into_iter()
        .map(|(pid, (name, ppid, pgid, ticks, rss))| {
            let was = before.get(&pid).copied().unwrap_or(0);
            ProcRow { pid, ppid, pgid, name, cpu_pct: ticks.saturating_sub(was) as f64 / tck / secs.max(0.001) * 100.0, rss_mb: rss as f64 / 1048576.0, gpu_mb: 0.0 }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn sample() -> (f64, Vec<ProcRow>) {
    let read = || std::fs::read_to_string("/proc/stat").ok().and_then(|t| parse_proc_stat(&t));
    let (s0, p0) = (read(), procs());
    let t0 = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let (s1, p1) = (read(), procs());
    let cpu = match (s0, s1) {
        (Some(a), Some(b)) => cpu_pct(a, b),
        _ => 0.0,
    };
    let before = p0.into_iter().map(|(pid, p)| (pid, p.3)).collect();
    (cpu, rows_between(&before, p1, t0.elapsed().as_secs_f64()))
}

#[cfg(windows)]
fn sample() -> (f64, Vec<ProcRow>) {
    let procs = || -> std::collections::HashMap<u32, (String, u64, u64)> {
        crate::os::processes().into_iter().filter_map(|(pid, name, _)| crate::os::proc_numbers(pid).map(|(ns, rss)| (pid, (name, ns, rss)))).collect()
    };
    let (s0, p0) = (crate::os::cpu_times(), procs());
    let t0 = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let (s1, p1) = (crate::os::cpu_times(), procs());
    let ns = t0.elapsed().as_nanos().max(1) as f64;
    let cpu = match (s0, s1) {
        (Some(a), Some(b)) => cpu_pct(a, b),
        _ => 0.0,
    };
    let rows = p1
        .into_iter()
        .map(|(pid, (name, t, rss))| {
            let before = p0.get(&pid).map_or(t, |p| p.1);
            ProcRow { pid, ppid: 0, pgid: pid, name, cpu_pct: t.saturating_sub(before) as f64 / ns * 100.0, rss_mb: rss as f64 / 1048576.0, gpu_mb: 0.0 }
        })
        .collect();
    (cpu, rows)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn sample() -> (f64, Vec<ProcRow>) {
    // macOS (and the rest): `ps` gives a decaying CPU average per process; the machine's use is
    // their sum over the cores. Spawned, not read in place: /bin/ps is setuid root, and libproc
    // refuses another user's CPU and RSS to this process (WindowServer's, which a busy look names).
    // It runs for a CPU look alone (a timed start, a timed lease's noise look every 10 s); the GPU
    // split names its processes from [`named`]
    let out = makepad_remote::group::command("/bin/ps").args(["-Ao", "pid=,ppid=,pgid=,pcpu=,rss=,comm="]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let rows: Vec<ProcRow> = text
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let pid = w.next()?.parse().ok()?;
            let ppid = w.next()?.parse().ok()?;
            let pgid = w.next()?.parse().ok()?;
            let cpu = w.next()?.parse().ok()?;
            let rss: f64 = w.next()?.parse().ok()?;
            let comm = w.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            Some(ProcRow { pid, ppid, pgid, name, cpu_pct: cpu, rss_mb: rss / 1024.0, gpu_mb: 0.0 })
        })
        .collect();
    let cores = std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0);
    let cpu = (rows.iter().map(|r| r.cpu_pct).sum::<f64>() / cores).min(100.0);
    (cpu, rows)
}

/// `makepad run makepad-app-calculator` rather than the kernel's 15-byte `makepad`: the
/// program's file name and its first words.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn label(pid: u32) -> Option<String> {
    let raw = argv_bytes(pid)?;
    let mut words = raw.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned());
    let exe = words.next()?;
    let mut out = exe.rsplit('/').next().unwrap_or(&exe).to_string();
    for w in words.take(3) {
        if w.starts_with('-') || out.len() + w.len() > 48 {
            break;
        }
        out.push(' ');
        out.push_str(w.rsplit('/').next().unwrap_or(&w));
    }
    Some(out)
}

/// The process's argv, NUL-separated.
#[cfg(target_os = "linux")]
fn argv_bytes(pid: u32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/cmdline")).ok()
}

/// The process's argv, NUL-separated: sysctl KERN_PROCARGS2 is `argc`, the exec path, NUL padding,
/// then argv (and the environment, which is cut off after argc words).
#[cfg(target_os = "macos")]
fn argv_bytes(pid: u32) -> Option<Vec<u8>> {
    extern "C" {
        fn sysctl(name: *const i32, namelen: u32, oldp: *mut u8, oldlenp: *mut usize, newp: *const u8, newlen: usize) -> i32;
    }
    // CTL_KERN 1, KERN_PROCARGS2 49
    let mib = [1i32, 49, pid as i32];
    let mut buf = vec![0u8; 64 << 10];
    let mut len = buf.len();
    if unsafe { sysctl(mib.as_ptr(), 3, buf.as_mut_ptr(), &mut len, std::ptr::null(), 0) } != 0 || len < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?).max(0) as usize;
    let rest = &buf[4..len];
    // skip the exec path and its NUL padding
    let path_end = rest.iter().position(|b| *b == 0)?;
    let start = path_end + rest[path_end..].iter().position(|b| *b != 0)?;
    let mut out = Vec::new();
    for w in rest[start..].split(|b| *b == 0).take(argc) {
        out.extend_from_slice(w);
        out.push(0);
    }
    Some(out)
}

/// Windows: the program's file name (another process's argv is not asked).
#[cfg(windows)]
pub fn label(pid: u32) -> Option<String> {
    crate::os::exe_name(pid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn label(_pid: u32) -> Option<String> {
    None
}

/// Whose a process is, by its process group or an ancestor's: `whose(pgid)` names a group's
/// owner ("" for the timed lease's own work, else a lease's tag); a process this hub started
/// (its own builds) is the hub's, with what the hub started; else what it runs under, when that
/// is another program (`under cargo < makepad-app-wm`); None: none of these (the OS, the user).
fn owner(rows: &std::collections::HashMap<u32, &ProcRow>, r: &ProcRow, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> Option<String> {
    match owner_of(rows, r, me, whose) {
        Owner::Group(w) | Owner::Hub(w) => Some(w),
        Owner::Nobody => under(rows, r),
    }
}

/// Whose a process is ([`owner_of`]).
enum Owner {
    /// a group `whose` names, by that name
    Group(String),
    /// the hub started it: `the hub's <what>`
    Hub(String),
    /// neither: the OS's, the user's (what it runs under: [`under`])
    Nobody,
}

/// The programs `r` runs under, below the box's init and session managers: its parent, and the
/// topmost one when that is another (`under cargo < makepad-app-wm`). A timed run's box said
/// `rustc 71%` (at up to 14 cores) and nothing said whose it was (11-10).
fn under(rows: &std::collections::HashMap<u32, &ProcRow>, r: &ProcRow) -> Option<String> {
    let manager = |p: &ProcRow| p.pid <= 1 || matches!(p.name.as_str(), "systemd" | "init" | "launchd");
    let mut chain: Vec<&ProcRow> = Vec::new();
    let mut at = r;
    // a parent chain is short; the bound only guards a cycle in a racy sample
    for _ in 0..64 {
        match rows.get(&at.ppid).filter(|p| !manager(p)) {
            Some(p) => {
                chain.push(p);
                at = p;
            }
            None => break,
        }
    }
    let name = |p: &ProcRow| label(p.pid).unwrap_or_else(|| p.name.clone());
    let (first, top) = (chain.first()?, chain.last()?);
    Some(if first.pid == top.pid { format!("under {}", name(first)) } else { format!("under {} < {}", name(first), name(top)) })
}

fn owner_of(rows: &std::collections::HashMap<u32, &ProcRow>, r: &ProcRow, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> Owner {
    let mut at = r;
    // a parent chain is short; the bound only guards a cycle in a racy sample
    for _ in 0..64 {
        if let Some(w) = whose(at.pgid) {
            return Owner::Group(w);
        }
        if at.ppid == me {
            let what = label(at.pid).unwrap_or_else(|| at.name.clone());
            return Owner::Hub(if crate::sync::prewarm::step_leader() == Some(at.pid) { format!("the hub's prewarm ({what})") } else { format!("the hub's {what}") });
        }
        match rows.get(&at.ppid) {
            Some(p) => at = p,
            None => break,
        }
    }
    Owner::Nobody
}

/// The programs of a build ([`compilers_among`]).
const COMPILERS: [&str; 13] = ["rustc", "cargo", "cc", "c++", "gcc", "g++", "clang", "cc1", "cc1plus", "collect2", "ld", "ld.lld", "mold"];

/// The box's processes, each with its CPU over a half-second look.
pub fn processes() -> Vec<ProcRow> {
    sample().1
}

/// Every process's pid, parent, group and name, without CPU or RSS, read in place: macOS's
/// `sysctl` process table, which holds other users' processes too (WindowServer), and the
/// executable's file name where libproc gives it. Elsewhere [`processes`].
#[cfg(target_os = "macos")]
pub fn named() -> Vec<ProcRow> {
    extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut u8, size: u32) -> i32;
    }
    kinfo_table().chunks_exact(KINFO_SIZE).filter(|k| k[36] != 5).filter_map(|k| {
        let int = |at: usize| i32::from_ne_bytes(k[at..at + 4].try_into().unwrap());
        let pid = int(40);
        (pid > 0).then_some(())?;
        // the name the exec gave it: p_comm, which keeps 16 bytes of it, and else the path the
        // process was exec'd by. A path resolved from its file (proc_pidpath) names whichever hard
        // link of the file the system resolved last: a JIT run's file is the loader's, which other
        // programs' links share (relax selfexe.rs), so it counts only where it begins with p_comm.
        let comm = kinfo_comm(k);
        let base = |full: &str| full.rsplit('/').next().unwrap_or(full).to_string();
        let mut path = [0u8; 4096];
        let n = unsafe { proc_pidpath(pid, path.as_mut_ptr(), path.len() as u32) };
        let resolved = (n > 0).then(|| base(&String::from_utf8_lossy(&path[..n as usize]))).filter(|r| r.starts_with(&comm));
        let name = match resolved {
            Some(r) => r,
            None if comm.len() < 16 => comm,
            None => crate::askpeer::args_of(pid as u32).map(|(exe, _)| base(&exe)).filter(|e| e.starts_with(&comm)).unwrap_or(comm),
        };
        Some(ProcRow { pid: pid as u32, ppid: int(560) as u32, pgid: int(564) as u32, name, ..Default::default() })
    }).collect()
}

/// `struct kinfo_proc` (sys/sysctl.h), 648 bytes on 64-bit macOS: p_starttime (a timeval) at 0,
/// p_stat at 36 (5: a zombie), p_pid at 40, p_comm[17] at 243, kp_eproc's e_ppid at 560 and
/// e_pgid at 564.
#[cfg(target_os = "macos")]
const KINFO_SIZE: usize = 648;

/// macOS's `sysctl` process table (KERN_PROC_ALL): a `struct kinfo_proc` per process, other
/// users' too; empty when the table cannot be read.
#[cfg(target_os = "macos")]
fn kinfo_table() -> Vec<u8> {
    extern "C" {
        fn sysctl(name: *const i32, namelen: u32, oldp: *mut u8, oldlenp: *mut usize, newp: *const u8, newlen: usize) -> i32;
    }
    let mib = [1, 14, 0]; // CTL_KERN, KERN_PROC, KERN_PROC_ALL
    loop {
        let mut len = 0usize;
        if unsafe { sysctl(mib.as_ptr(), 3, std::ptr::null_mut(), &mut len, std::ptr::null(), 0) } != 0 {
            return Vec::new();
        }
        len += len / 8 + KINFO_SIZE * 16;
        let mut buf = vec![0u8; len];
        // (the table grew between the two calls: ask again)
        if unsafe { sysctl(mib.as_ptr(), 3, buf.as_mut_ptr(), &mut len, std::ptr::null(), 0) } == 0 {
            buf.truncate(len);
            return buf;
        }
    }
}

/// A `struct kinfo_proc`'s p_comm: the first 16 bytes of the name its exec gave it.
#[cfg(target_os = "macos")]
fn kinfo_comm(k: &[u8]) -> String {
    let comm = &k[243..260];
    String::from_utf8_lossy(&comm[..comm.iter().position(|b| *b == 0).unwrap_or(comm.len())]).into_owned()
}

/// When the newest live process named `comm` (its p_comm) started, in µs since the epoch as
/// `makepad_remote::group::start_time` gives a process's start. The process table's start holds
/// for another user's process too (the window server's), which libproc does not read. None: no
/// such process.
#[cfg(target_os = "macos")]
pub fn newest_start(comm: &str) -> Option<u64> {
    kinfo_table().chunks_exact(KINFO_SIZE).filter(|k| k[36] != 5 && kinfo_comm(k) == comm).map(|k| {
        let sec = i64::from_ne_bytes(k[0..8].try_into().unwrap());
        let usec = i32::from_ne_bytes(k[8..12].try_into().unwrap());
        sec.max(0) as u64 * 1_000_000 + usec.max(0) as u64
    }).max()
}

/// [`processes`]: off macOS the process table comes with CPU and RSS in place.
#[cfg(not(target_os = "macos"))]
pub fn named() -> Vec<ProcRow> {
    processes()
}

/// This process and launchd (another user's) are in the table with their parent and group.
#[cfg(all(test, target_os = "macos"))]
#[test]
fn the_named_table_holds_other_users_processes_without_a_spawn() {
    extern "C" {
        fn getppid() -> i32;
        fn getpgrp() -> i32;
    }
    let rows = named();
    let me = rows.iter().find(|r| r.pid == std::process::id()).expect("this process");
    assert_eq!((me.ppid, me.pgid), (unsafe { getppid() } as u32, unsafe { getpgrp() } as u32));
    assert!(!me.name.is_empty());
    assert!(rows.iter().any(|r| r.pid == 1 && r.name == "launchd"), "another user's process: {:?}", rows.iter().find(|r| r.pid == 1));
    // its start too, on the clock a process's own start is read on: launchd's is the boot's
    let mine = makepad_remote::group::start_time(std::process::id()).expect("this process's start");
    let boot = newest_start("launchd").expect("launchd's start");
    assert!(boot > 0 && boot < mine, "launchd started at {boot} µs, this process at {mine} µs");
    assert_eq!(newest_start("no-such-process-name"), None);
}

/// The compilers among a look's `rows` ([`processes`]; `me`: the hub's pid) that run outside
/// every lease (`whose(pgid)` names a lease's groups) at 1% of a core or more, the hub's pre-warm
/// apart (a lease start stops it): each with its pid, whose it is or what it runs under, and its
/// CPU, the busiest first (`rustc pid 812 (under cargo < makepad run app) at 808% of a core`). A
/// timed start waits for them: a box's app took its deployment's move and rebuilt the makepad it
/// runs at up to 14 cores, in no lease, under three timed runs in 20 min (11-10).
pub fn compilers_among(rows: &[ProcRow], me: u32, whose: &dyn Fn(u32) -> Option<String>) -> Vec<String> {
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut busy: Vec<(f64, String)> = rows.iter().filter(|r| r.pid != me && r.cpu_pct >= 1.0 && COMPILERS.contains(&r.name.as_str())).filter_map(|r| {
        let whose = match owner_of(&by_pid, r, me, whose) {
            Owner::Group(_) => return None,
            Owner::Hub(w) if w.starts_with("the hub's prewarm (") => return None,
            Owner::Hub(w) => w,
            Owner::Nobody => under(&by_pid, r).unwrap_or_else(|| "of no lease".to_string()),
        };
        Some((r.cpu_pct, format!("{} pid {} ({whose}) at {:.0}% of a core", r.name, r.pid, r.cpu_pct)))
    }).collect();
    busy.sort_by(|a, b| b.0.total_cmp(&a.0));
    busy.into_iter().map(|(_, s)| s).collect()
}

/// What runs outside every lease among a look's `rows` ([`processes`]; `me`: the hub's pid; `whose`
/// names a lease's groups), the hub's pre-warm apart (a lease start stops it): its CPU in all (%
/// of one core) and the busiest three (`WindowServer 34%`, `rustc (the hub's cargo build) 180%`).
/// A timed start waits while it is [`crate::lease::master::NOISY_CPU_PCT`] or more.
pub fn outside_among(rows: &[ProcRow], me: u32, whose: &dyn Fn(u32) -> Option<String>) -> (f64, Vec<String>) {
    outside_where(rows, &|_| true, me, whose)
}

/// [`outside_among`] of the processes `pick` takes among `rows`, each placed by its parents
/// among all of them.
pub fn outside_where(rows: &[ProcRow], pick: &dyn Fn(&ProcRow) -> bool, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> (f64, Vec<String>) {
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut busy: Vec<(f64, String)> = rows.iter().filter(|r| r.pid != me && r.cpu_pct >= 1.0 && pick(r)).filter_map(|r| match owner_of(&by_pid, r, me, whose) {
        Owner::Group(_) => None,
        Owner::Hub(w) if w.starts_with("the hub's prewarm (") => None,
        Owner::Hub(w) => Some((r.cpu_pct, format!("{} ({w}) {:.0}%", r.name, r.cpu_pct))),
        Owner::Nobody => Some((r.cpu_pct, format!("{} {:.0}%", r.name, r.cpu_pct))),
    }).collect();
    busy.sort_by(|a, b| b.0.total_cmp(&a.0));
    (busy.iter().map(|(cpu, _)| cpu).sum(), busy.into_iter().take(3).map(|(_, said)| said).collect())
}

/// The CPU (% of one core, a half-second look) of each of `pids` that runs.
pub fn cpu_of(pids: &[u32]) -> Vec<(u32, f64)> {
    let (_, rows) = sample();
    rows.into_iter().filter(|r| pids.contains(&r.pid)).map(|r| (r.pid, r.cpu_pct)).collect()
}

/// A process outside the box's leases is named once it uses this many cores on average.
#[cfg(not(windows))]
const OUTSIDE_MIN_CORES: f64 = 0.5;
/// and at most this many of them, the busiest
#[cfg(not(windows))]
const OUTSIDE_TOP: usize = 3;

/// The box's CPU by whose it was since the previous call (the master's caps poll, every 10 s),
/// cores on average: each lease's (`lease_of(pgid)` names a lease run's process group: what runs
/// in it or under it is that lease's), the busiest first, and the busiest processes outside the
/// leases (what it is, pid, cores), at least [`OUTSIDE_MIN_CORES`] each, at most [`OUTSIDE_TOP`];
/// what the hub started outside a lease is named as the hub's. A box measured busier than its
/// leases' charges says what took the rest: a lease past its charge or work of no lease's (a box
/// charged a few cores measured 7-8 more the whole of a live launch's 9 min wait, and nothing said
/// what, W4322, 11-08). Windows: none (a process's parent is not known there, so a lease's
/// children cannot be told from the rest).
#[cfg(target_os = "linux")]
pub fn cpu_split(lease_of: &dyn Fn(u32) -> Option<String>) -> CpuSplit {
    use std::collections::HashMap;
    static LAST: std::sync::Mutex<Option<(std::time::Instant, HashMap<u32, u64>)>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    let after = procs();
    let ticks = after.iter().map(|(pid, p)| (*pid, p.3)).collect();
    let Some((then, before)) = LAST.lock().unwrap().replace((now, ticks)) else { return CpuSplit::default() };
    split_rows(&rows_between(&before, after, now.duration_since(then).as_secs_f64()), lease_of)
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn cpu_split(lease_of: &dyn Fn(u32) -> Option<String>) -> CpuSplit {
    // (`ps` gives each process's CPU as a decaying average: no span of its own)
    split_rows(&sample().1, lease_of)
}
#[cfg(windows)]
pub fn cpu_split(_lease_of: &dyn Fn(u32) -> Option<String>) -> CpuSplit {
    CpuSplit::default()
}

/// [`cpu_split`]'s answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuSplit {
    /// (lease, cores), the busiest first
    pub leases: Vec<(String, f64)>,
    /// (what, pid, cores), the busiest first
    pub outside: Vec<(String, u32, f64)>,
    /// (pid, app): the apps of ours that run outside every lease and the hub, busy or not, by the
    /// registry's label ([`apps_outside`])
    pub apps: Vec<(u32, String)>,
    /// (lease, cores, what): the compilers ([`COMPILERS`]) at 1% of a core or more in or under
    /// each lease's groups, with the crate the busiest one builds ([`build_crate`], else its
    /// name): a live session's app's builds, a WM's cargo builds of the apps it hosts among them
    pub builds: Vec<(String, f64, String)>,
}

/// [`cpu_split`] of a look's rows.
#[cfg(not(windows))]
fn split_rows(rows: &[ProcRow], lease_of: &dyn Fn(u32) -> Option<String>) -> CpuSplit {
    let me = std::process::id();
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut out = CpuSplit::default();
    // (lease, cores, its busiest compiler: pid, % of a core, name)
    let mut builds: Vec<(String, f64, (u32, f64, String))> = Vec::new();
    for r in rows.iter().filter(|r| r.pid != me && r.cpu_pct > 0.0) {
        let what = match owner_of(&by_pid, r, me, lease_of) {
            Owner::Group(lease) => {
                if r.cpu_pct >= 1.0 && COMPILERS.contains(&r.name.as_str()) {
                    match builds.iter_mut().find(|(l, ..)| *l == lease) {
                        Some((_, cores, busiest)) => {
                            *cores += r.cpu_pct / 100.0;
                            if r.cpu_pct > busiest.1 { *busiest = (r.pid, r.cpu_pct, r.name.clone()); }
                        }
                        None => builds.push((lease.clone(), r.cpu_pct / 100.0, (r.pid, r.cpu_pct, r.name.clone()))),
                    }
                }
                match out.leases.iter_mut().find(|(l, _)| *l == lease) {
                    Some((_, cores)) => *cores += r.cpu_pct / 100.0,
                    None => out.leases.push((lease, r.cpu_pct / 100.0)),
                }
                continue;
            }
            // (named only when it is said: a look has hundreds of these)
            _ if r.cpu_pct < OUTSIDE_MIN_CORES * 100.0 => continue,
            Owner::Hub(what) => what,
            Owner::Nobody => {
                let what = label(r.pid).unwrap_or_else(|| r.name.clone());
                under(&by_pid, r).map_or_else(|| what.clone(), |u| format!("{what} ({u})"))
            }
        };
        out.outside.push((crate::proto::clean(&what, 60), r.pid, r.cpu_pct / 100.0));
    }
    out.leases.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.outside.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.cmp(&b.1)));
    out.outside.truncate(OUTSIDE_TOP);
    out.apps = apps_outside(rows, &by_pid, me, lease_of);
    // (one argv read per lease that builds: its busiest compiler's)
    out.builds = builds.into_iter().map(|(lease, cores, (pid, _, name))| (lease, cores, crate_of(pid).unwrap_or(name))).collect();
    out.builds.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// What compiler `pid` builds, by its argv ([`build_crate`]).
#[cfg(not(windows))]
fn crate_of(pid: u32) -> Option<String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let raw = argv_bytes(pid)?;
        let words: Vec<String> = raw.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
        build_crate(&words)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    { let _ = pid; None }
}

/// The crate a compiler's argv builds: rustc's `--crate-name`, else cargo's package (`-p`,
/// `--package`) or binary (`--bin`); none when it names none (a linker).
#[cfg(any(target_os = "linux", target_os = "macos", all(test, not(windows))))]
fn build_crate(words: &[String]) -> Option<String> {
    let after = |flags: &[&str]| {
        words.windows(2).find(|w| flags.contains(&w[0].as_str())).map(|w| w[1].clone())
            .or_else(|| words.iter().find_map(|w| flags.iter().find_map(|f| w.strip_prefix(*f).and_then(|v| v.strip_prefix('=')).map(str::to_string))))
    };
    after(&["--crate-name"]).or_else(|| after(&["-p", "--package"])).or_else(|| after(&["--bin"])).filter(|c| !c.is_empty())
}

/// The windowed apps of ours (the registry's WM-launchable rows) among a look's `rows` that run
/// outside every lease and the hub: a person's, or one an agent opened without a lease (an XR
/// session or a viewer on one holds its box, `lease::hand`). Only a process named like one of
/// ours (`makepad`, or an app's binary: the kernel cuts names at 15 bytes) has its argv read; a
/// devhub client naming an app is none.
#[cfg(all(any(target_os = "linux", target_os = "macos"), feature = "checkout"))]
fn apps_outside(rows: &[ProcRow], by_pid: &std::collections::HashMap<u32, &ProcRow>, me: u32, lease_of: &dyn Fn(u32) -> Option<String>) -> Vec<(u32, String)> {
    let ours = |name: &str| name.starts_with("makepad") || makepad_app_registry::apps().iter().any(|a| a.bin == name || name.len() >= 15 && a.bin.starts_with(name));
    let mut apps: Vec<(u32, String)> = rows.iter().filter(|r| r.pid != me && ours(&r.name) && matches!(owner_of(by_pid, r, me, lease_of), Owner::Nobody)).filter_map(|r| {
        let argv = argv_bytes(r.pid)?;
        let words: Vec<String> = argv.split(|b| *b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect();
        if words.iter().any(|w| w == "devhub") { return None; }
        let app = makepad_app_registry::machine::app_in_command(words.iter().map(String::as_str)).filter(|a| a.wm_launchable)?;
        Some((r.pid, app.label.to_string()))
    }).collect();
    apps.sort();
    apps
}
#[cfg(all(not(windows), not(all(any(target_os = "linux", target_os = "macos"), feature = "checkout"))))]
fn apps_outside(_rows: &[ProcRow], _by_pid: &std::collections::HashMap<u32, &ProcRow>, _me: u32, _lease_of: &dyn Fn(u32) -> Option<String>) -> Vec<(u32, String)> {
    Vec::new()
}

/// The processes [`cpu_split`] names outside the leases, said: `dumper pid 812 (1.0 cores), ...`.
pub fn outside_said(busy: &[(String, u32, f64)]) -> String {
    busy.iter().map(|(what, pid, cores)| format!("{what} pid {pid} ({cores:.1} cores)")).collect::<Vec<_>>().join(", ")
}

/// The `top` answer: a machine line, a line per GPU, then the `n` busiest processes.
pub fn top(machine: &str, n: usize) -> Vec<String> {
    let (cpu, mut rows) = sample();
    for (pid, mb) in gpu_procs() {
        if let Some(r) = rows.iter_mut().find(|r| r.pid == pid) {
            r.gpu_mb = mb;
        }
    }
    let m = crate::procinfo::machine();
    let disk = crate::lease::caps::disk_space(&crate::lease::caps::LeaseConf::load().work).unwrap_or((0, 0));
    let mut out = vec![format!(
        "{{\"machine\":{},\"cpu_pct\":{cpu:.1},\"cores\":{},\"load1\":{:.2},\"mem_total_mb\":{},\"mem_avail_mb\":{},\"disk_free_gb\":{:.1},\"disk_total_gb\":{:.1}}}",
        json_str(machine),
        m.cpus,
        m.load1,
        m.mem_total_mb,
        m.mem_avail_mb,
        disk.0 as f64 / crate::lease::caps::GB,
        disk.1 as f64 / crate::lease::caps::GB
    )];
    for (i, g) in gpu_use().iter().enumerate() {
        out.push(format!("{{\"gpu\":{i},\"name\":{},\"util_pct\":{:.0},\"mem_used_gb\":{:.1},\"mem_total_gb\":{:.1},\"temp_c\":{:.0},\"power_w\":{:.0}}}", json_str(&g.name), g.util_pct, g.mem_used_gb, g.mem_total_gb, g.temp_c, g.power_w));
    }
    rows.sort_by(|a, b| (b.cpu_pct + b.gpu_mb / 1000.0).total_cmp(&(a.cpu_pct + a.gpu_mb / 1000.0)));
    // the box's resident apps are listed busy or not, after the busiest: the WM is what an idle-CPU
    // hunt looks for, and a busy box's short list left it out (11-03)
    let services = service_pids();
    let mut i = 0;
    rows.retain(|r| {
        i += 1;
        i <= n || services.contains(&r.pid) || label(r.pid).is_some_and(|l| l.contains("makepad-app-wm"))
    });
    for r in rows.iter_mut() {
        if let Some(l) = label(r.pid) {
            r.name = l;
        }
    }
    for r in rows.iter() {
        out.push(format!("{{\"pid\":{},\"name\":{},\"cpu_pct\":{:.1},\"rss_mb\":{:.0},\"gpu_mb\":{:.0}}}", r.pid, json_str(&crate::proto::clean(&r.name, 60)), r.cpu_pct, r.rss_mb, r.gpu_mb));
    }
    out
}

/// The pids of the services the box's WM supervises (`wm/<name>.state`).
fn service_pids() -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir(makepad_network::fleet::makepad_home().join("wm")) else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "state"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok()?.lines().find_map(|l| l.strip_prefix("pid = ")?.trim().parse().ok()))
        .collect()
}

/// `top` lines as text.
pub fn render(lines: &[String], out: &mut String) {
    let num = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(crate::lease::num).unwrap_or(0.0);
    for l in lines {
        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if v.get("machine").is_some() {
            out.push_str(&format!(
                "{}  cpu {:.0}% of {} cores  load {:.1}  ram {:.1}/{:.0} GB used  disk {:.0} GB free of {:.0}\n",
                s("machine"),
                num(&v, "cpu_pct"),
                num(&v, "cores"),
                num(&v, "load1"),
                (num(&v, "mem_total_mb") - num(&v, "mem_avail_mb")) / 1024.0,
                num(&v, "mem_total_mb") / 1024.0,
                num(&v, "disk_free_gb"),
                num(&v, "disk_total_gb")
            ));
        } else if v.get("gpu").is_some() {
            out.push_str(&format!("  gpu{} {}  {:.0}%  vram {:.1}/{:.0} GB  {:.0} C  {:.0} W\n", num(&v, "gpu"), s("name"), num(&v, "util_pct"), num(&v, "mem_used_gb"), num(&v, "mem_total_gb"), num(&v, "temp_c"), num(&v, "power_w")));
        } else if v.get("pid").is_some() {
            let gpu = if num(&v, "gpu_mb") > 0.0 { format!("  gpu {:.0} MB", num(&v, "gpu_mb")) } else { String::new() };
            out.push_str(&format!("  {:>8} {:<40} cpu {:>5.1}%  rss {:>6.0} MB{gpu}\n", num(&v, "pid"), s("name"), num(&v, "cpu_pct"), num(&v, "rss_mb")));
        } else if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            out.push_str(&format!("  error: {}\n", crate::proto::clean(e, 200)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_and_cpu_numbers() {
        let g = parse_gpu_use("NVIDIA GeForce GPU-A, 37, 4096, 25000, 51, 120.33\n");
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].util_pct, g[0].mem_used_gb, g[0].temp_c), (37.0, 4.0, 51.0));
        assert!(parse_gpu_use("").is_empty());
        let clients = |a: u64, b: u64| format!(r#"+-o AGXDeviceUserClient
    "IOUserClientCreator" = "pid 5000030, WindowServer"
    "AppUsage" = ({{"accumulatedGPUTime"={a}}},{{"accumulatedGPUTime"=1000}})
+-o AGXDeviceUserClient
    "IOUserClientCreator" = "pid 5000031, timedrun"
    "AppUsage" = ({{"accumulatedGPUTime"={b}}})
+-o AGXDeviceUserClient
    "IOUserClientCreator" = "pid 5000032, helper"
    "AppUsage" = ()
+-o AGXDeviceUserClient
    "IOUserClientCreator" = "pid 5000030, WindowServer"
    "AppUsage" = ({{"accumulatedGPUTime"=2000}})
"#);
        let (mut before, mut after) = (parse_gpu_times(&clients(0, 0)), parse_gpu_times(&clients(300_000_000, 9_000_000_000)));
        assert_eq!(before, vec![(5_000_030, 3000), (5_000_031, 0), (5_000_032, 0)], "two clients of one pid summed, one with no use at 0");
        before.extend([(5_000_033, 0), (5_000_000, 0), (5_000_035, 0)]);
        after.extend([(5_000_033, 2_000_000_000), (5_000_000, 5_000_000_000), (5_000_035, 6_000_000_000), (5_000_036, 7_000_000_000)]);
        let rates = gpu_rates(&before, &after, Duration::from_secs(10));
        assert_eq!(rates, vec![(5_000_030, 3.0), (5_000_031, 90.0), (5_000_033, 20.0), (5_000_000, 50.0), (5_000_035, 60.0)], "nanosecond deltas become GPU percentages; a new pid and a zero delta are left out");
        let row = |pid: u32, pgid: u32, name: &str| ProcRow { pid, ppid: 1, pgid, name: name.into(), ..Default::default() };
        let rows = [row(5_000_030, 5_000_030, "WindowServer"), row(5_000_031, 5_000_040, "timedrun"), row(5_000_032, 5_000_032, "helper"), row(5_000_033, 5_000_041, "other-run"), row(5_000_000, 5_000_000, "hub")];
        let whose = |pgid: u32| match pgid { 5_000_040 => Some(String::new()), 5_000_041 => Some("L2 other-lane".into()), _ => None };
        let (pct, by) = gpu_split(&rates, &rows, 5_000_000, &whose);
        assert_eq!((pct, by.clone()), (23.0, vec![("other-run (L2 other-lane)".to_string(), 5_000_033, 20.0), ("WindowServer".to_string(), 5_000_030, 3.0)]), "own groups, the hub and gone pids are left out; another lease is named, busiest first, and the rest summed");
        assert_eq!(gpu_said(&by), "other-run (L2 other-lane) pid 5000033 at 20% of the GPU, WindowServer pid 5000030 at 3% of the GPU");
        let a = parse_proc_stat("cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        let b = parse_proc_stat("cpu  200 0 200 1400 0 0 0 0 0 0\n").unwrap();
        assert_eq!(a, (200, 1000));
        assert!((cpu_pct(a, b) - 25.0).abs() < 0.01);
        let mut s = String::new();
        render(&top("t", 3), &mut s);
        assert!(s.starts_with("t  cpu "), "{s}");
    }

    /// What a box ran outside its leases: a lease's run and what runs under it are the lease's;
    /// what the hub started outside a lease is the hub's; the rest by name and pid, busiest first,
    /// at least half a core each, three at most.
    #[cfg(not(windows))]
    #[test]
    fn the_busiest_processes_outside_the_leases_are_named() {
        let me = std::process::id();
        let row = |pid: u32, ppid: u32, pgid: u32, name: &str, cpu_pct: f64| ProcRow { pid, ppid, pgid, name: name.into(), cpu_pct, ..Default::default() };
        let rows = [
            // a lease's run (group 4000000) and a compiler under it in a group of its own
            row(4000000, 1, 4000000, "bash", 0.0),
            row(4000001, 4000000, 4000001, "rustc", 790.0),
            // the hub's own build
            row(4000010, me, 4000010, "cargo", 120.0),
            // the box's own work, one idle
            row(4000020, 1, 4000020, "dumper", 98.0),
            row(4000021, 1, 4000021, "indexer", 260.0),
            row(4000022, 1, 4000022, "wm", 3.0),
            row(4000023, 1, 4000023, "sweeper", 60.0),
            // a compiler of no lease's and not the hub's, under a cargo another program started
            row(4000030, 1, 4000030, "session", 0.0),
            row(4000031, 4000030, 4000031, "cargo", 0.0),
            row(4000032, 4000031, 4000031, "rustc", 40.0),
            row(4000033, 4000030, 4000030, "lone", 40.0),
        ];
        let lease_of = |pgid: u32| (pgid == 4000000).then(|| "L7".to_string());
        let split = split_rows(&rows, &lease_of);
        assert_eq!(split.leases, vec![("L7".to_string(), 7.9)], "the compiler under the lease's run is the lease's");
        // and its builds: a live session's app's keep it from its idle end (no argv for a pid of
        // this table: its name says it)
        assert_eq!(split.builds, vec![("L7".to_string(), 7.9, "rustc".to_string())], "the lease's compilers at work, not the idle bash");
        let words = |w: &str| w.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(build_crate(&words("rustc --edition=2021 --crate-name makepad_widgets --crate-type lib")).as_deref(), Some("makepad_widgets"));
        assert_eq!(build_crate(&words("cargo run --release -p makepad-app-notes")).as_deref(), Some("makepad-app-notes"));
        assert_eq!(build_crate(&words("cargo build --package=makepad-app-clock")).as_deref(), Some("makepad-app-clock"));
        assert_eq!(build_crate(&words("ld -o out a.o")), None, "a linker names no crate");
        let busy = split.outside;
        let pids: Vec<u32> = busy.iter().map(|b| b.1).collect();
        assert_eq!(pids, vec![4000021, 4000010, 4000020], "{busy:?}");
        assert_eq!(busy[0].0, "indexer");
        assert!(busy[1].0.starts_with("the hub's "), "{busy:?}");
        assert_eq!(outside_said(&busy[2..]), "dumper pid 4000020 (1.0 cores)");
        let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
        let said = |pid: u32| owner(&by_pid, by_pid[&pid], me, &lease_of);
        assert_eq!(said(4000032).as_deref(), Some("under cargo < session"), "what an unowned process runs under");
        assert_eq!(said(4000033).as_deref(), Some("under session"));
        assert_eq!(said(4000020), None, "the box's own work, under its init");
        assert_eq!(said(4000001).as_deref(), Some("L7"));
        // what a timed start waits for: compilers of no lease's, busiest first, each with whose it is
        assert_eq!(compilers_among(&rows, me, &lease_of), vec![
            "cargo pid 4000010 (the hub's cargo) at 120% of a core".to_string(),
            "rustc pid 4000032 (under cargo < session) at 40% of a core".to_string(),
        ], "not the lease's compiler, an idle one or other programs");

        // a timed lease's exact look between two of its looks 10 s apart (a tick: 0.1% of a core):
        // pids past any real one
        let hub = 5_100_000;
        let p = |name: &str, ppid: u32, pgid: u32, start: u64, own: u64, reaped: u64| ProcTicks { name: name.into(), ppid, pgid, start, own, reaped };
        let at = Instant::now();
        let a = Ticks { at, per_s: 100.0, cpu: Some((10_000, 100)), procs: [
            (1, p("systemd", 0, 1, 0, 0, 1000)),
            (hub, p("devhub", 1, hub, 0, 500, 0)),
            (5_100_005, p("wm", 1, 5_100_005, 0, 300, 0)),
            (5_100_010, p("makepad-app-aihub", 5_100_005, 5_100_010, 0, 100, 200)),
            (5_100_011, p("nvidia-smi", 5_100_010, 5_100_011, 0, 5, 0)),
            (5_100_020, p("bash", hub, 5_100_020, 0, 50, 0)),
            (5_100_030, p("bash", hub, 5_100_030, 0, 0, 0)),
            (5_100_040, p("old", 1, 5_100_040, 7, 900, 0)),
        ].into_iter().collect() };
        let b = Ticks { at: at + Duration::from_secs(10), per_s: 100.0, cpu: Some((10_560, 142)), procs: [
            // an init's waited-for count holds anyone's orphans: not its own
            (1, p("systemd", 0, 1, 0, 0, 5000)),
            (hub, p("devhub", 1, hub, 0, 520, 0)),
            (5_100_005, p("wm", 1, 5_100_005, 0, 346, 0)),
            // its nvidia-smi seen at the first look ended after 2 more ticks; others came and went
            // (128 ticks): 135 more waited for, 5 of them counted at the first look already
            (5_100_010, p("makepad-app-aihub", 5_100_005, 5_100_010, 0, 140, 335)),
            (5_100_020, p("bash", hub, 5_100_020, 0, 250, 0)),
            (5_100_030, p("bash", hub, 5_100_030, 0, 30, 0)),
            // the pid again, another process: all its CPU is of the span
            (5_100_040, p("new", 1, 5_100_040, 9, 10, 0)),
            (5_100_050, p("fresh", 1, 5_100_050, 0, 7, 0)),
            (5_100_060, p("git", hub, 5_100_060, 0, 15, 0)),
        ].into_iter().collect() };
        let whose = |pgid: u32| match pgid { 5_100_020 => Some(String::new()), 5_100_030 => Some("L2 other".to_string()), _ => None };
        let (o, rows) = outside_between(&a, &b, hub, &whose);
        let tenth = |x: f64| format!("{x:.1}");
        let by: Vec<(&str, String, String)> = o.by.iter().map(|(w, cpu, kids)| (w.as_str(), tenth(*cpu), tenth(*kids))).collect();
        let want = |w: &'static str, cpu: &str, kids: &str| (w, cpu.to_string(), kids.to_string());
        assert_eq!(by, vec![
            want("makepad-app-aihub (under wm)", "17.0", "13.0"),
            want("wm", "4.6", "0.0"),
            want("bash (L2 other)", "3.0", "0.0"),
            want("git (the hub's git)", "1.5", "0.0"),
            want("new", "1.0", "0.0"),
            want("fresh", "0.7", "0.0"),
        ], "each program outside it with its children's part, another lease's and the hub's children named; not the timed lease's own, an init or the hub");
        assert_eq!((tenth(o.secs), tenth(o.hub)), ("10.0".to_string(), "2.0".to_string()), "the hub's own apart");
        assert_eq!(o.unseen.map(|(rest, irq)| (tenth(rest), tenth(irq))), Some(("6.2".to_string(), "4.2".to_string())), "the box's 56% less all 49.8% the processes showed; interrupts among it");
        assert_eq!(rows.iter().find(|r| r.pid == 5_100_020).map(|r| tenth(r.cpu_pct)).as_deref(), Some("20.0"), "the span's rows: each process's own CPU");
        assert_eq!(outside_between(&a, &Ticks { cpu: None, ..b.clone() }, hub, &whose).0.unseen, None, "a box that tells no CPU of its own (a Mac's ps)");
        // a Mac's ps: its CPU time so far, in hundredths of a second
        assert_eq!(cpu_time_cs("0:02.85"), Some(285));
        assert_eq!(cpu_time_cs("622:33.98"), Some(3_735_398));
        assert_eq!(cpu_time_cs("1-02:03:04.50"), Some((86_400 + 2 * 3600 + 3 * 60 + 4) * 100 + 50));
        assert_eq!(cpu_time_cs("00:01:05"), Some(6_500));
        assert_eq!(cpu_time_cs("-"), None);
        assert_eq!(parse_proc_busy_irq("cpu  100 0 100 800 0 30 12 0 0 0\n"), Some((242, 42)));
    }
}

//! `top`: what a node is busy with right now, for an agent in one call: CPU use (all cores, over
//! a short sample), load, RAM, disk, each GPU's use, memory and temperature, and the busiest
//! processes (CPU over the same sample, RSS, GPU memory). Read from the OS (`/proc`,
//! `nvidia-smi`; macOS `ps`), nothing is asked of the processes.

use makepad_remote::json_str;

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

#[derive(Clone, Debug, Default)]
struct ProcRow {
    pid: u32,
    /// its parent (Windows: unknown, 0)
    ppid: u32,
    /// its process group (Windows: itself)
    pgid: u32,
    name: String,
    cpu_pct: f64,
    rss_mb: f64,
    gpu_mb: f64,
}

/// Every process: pid -> (name, parent, group, CPU ticks so far, RSS bytes).
#[cfg(target_os = "linux")]
fn procs() -> std::collections::HashMap<u32, (String, u32, u32, u64, u64)> {
    let mut out = std::collections::HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[a + 1..b].to_string();
        let f: Vec<&str> = stat[b + 2..].split_whitespace().collect();
        let num = |i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
        out.insert(pid, (name, num(1) as u32, num(2) as u32, num(11) + num(12), num(21) * 4096));
    }
    out
}

/// The kernel's CPU ticks per second.
#[cfg(target_os = "linux")]
fn ticks_per_s() -> f64 {
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
    // their sum over the cores
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
/// (its own builds) is the hub's, with what the hub started; None: neither (the OS, the user).
fn owner(rows: &std::collections::HashMap<u32, &ProcRow>, r: &ProcRow, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> Option<String> {
    match owner_of(rows, r, me, whose) {
        Owner::Group(w) | Owner::Hub(w) => Some(w),
        Owner::Nobody => None,
    }
}

/// Whose a process is ([`owner_of`]).
enum Owner {
    /// a group `whose` names, by that name
    Group(String),
    /// the hub started it: `the hub's <what>`
    Hub(String),
    /// neither: the OS's, the user's
    Nobody,
}

fn owner_of(rows: &std::collections::HashMap<u32, &ProcRow>, r: &ProcRow, me: u32, whose: &dyn Fn(u32) -> Option<String>) -> Owner {
    let mut at = r;
    // a parent chain is short; the bound only guards a cycle in a racy sample
    for _ in 0..64 {
        if let Some(w) = whose(at.pgid) {
            return Owner::Group(w);
        }
        if at.ppid == me {
            return Owner::Hub(format!("the hub's {}", label(at.pid).unwrap_or_else(|| at.name.clone())));
        }
        match rows.get(&at.ppid) {
            Some(p) => at = p,
            None => return Owner::Nobody,
        }
    }
    Owner::Nobody
}

/// The CPU (% of one core, a half-second look) of each of `pids` that runs.
pub fn cpu_of(pids: &[u32]) -> Vec<(u32, f64)> {
    let (_, rows) = sample();
    rows.into_iter().filter(|r| pids.contains(&r.pid)).map(|r| (r.pid, r.cpu_pct)).collect()
}

/// What else the box ran: its load and the processes outside the timed lease's (`whose` says ""
/// for its groups and their children) and this hub, using at least 1% of a core, by CPU (% of
/// one core), the busiest first, each with its owner when known: another lease, or the hub's own
/// work (`rustc (L12 a-lane)`, `rustc (the hub's cargo build)`). A timed lease's report says
/// what shared its box (a Mac's Spotlight indexing what the last builds wrote; a rustc at 145%
/// that the report could not place, 11-03).
pub fn outside(whose: &dyn Fn(u32) -> Option<String>) -> (f64, Vec<(String, f64)>) {
    let (_, rows) = sample();
    let me = std::process::id();
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut v: Vec<(String, f64)> = rows.iter().filter(|r| r.pid != me && r.cpu_pct >= 1.0).filter_map(|r| match owner(&by_pid, r, me, whose) {
        Some(w) if w.is_empty() => None,
        Some(w) => Some((format!("{} ({w})", r.name), r.cpu_pct)),
        None => Some((r.name.clone(), r.cpu_pct)),
    }).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    (crate::procinfo::machine().load1, v)
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
}

/// [`cpu_split`] of a look's rows.
#[cfg(not(windows))]
fn split_rows(rows: &[ProcRow], lease_of: &dyn Fn(u32) -> Option<String>) -> CpuSplit {
    let me = std::process::id();
    let by_pid: std::collections::HashMap<u32, &ProcRow> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut out = CpuSplit::default();
    for r in rows.iter().filter(|r| r.pid != me && r.cpu_pct > 0.0) {
        let what = match owner_of(&by_pid, r, me, lease_of) {
            Owner::Group(lease) => {
                match out.leases.iter_mut().find(|(l, _)| *l == lease) {
                    Some((_, cores)) => *cores += r.cpu_pct / 100.0,
                    None => out.leases.push((lease, r.cpu_pct / 100.0)),
                }
                continue;
            }
            Owner::Hub(what) => what,
            Owner::Nobody => label(r.pid).unwrap_or_else(|| r.name.clone()),
        };
        if r.cpu_pct >= OUTSIDE_MIN_CORES * 100.0 {
            out.outside.push((crate::proto::clean(&what, 60), r.pid, r.cpu_pct / 100.0));
        }
    }
    out.leases.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.outside.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.1.cmp(&b.1)));
    out.outside.truncate(OUTSIDE_TOP);
    out
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
        ];
        let lease_of = |pgid: u32| (pgid == 4000000).then(|| "L7".to_string());
        let split = split_rows(&rows, &lease_of);
        assert_eq!(split.leases, vec![("L7".to_string(), 7.9)], "the compiler under the lease's run is the lease's");
        let busy = split.outside;
        let pids: Vec<u32> = busy.iter().map(|b| b.1).collect();
        assert_eq!(pids, vec![4000021, 4000010, 4000020], "{busy:?}");
        assert_eq!(busy[0].0, "indexer");
        assert!(busy[1].0.starts_with("the hub's "), "{busy:?}");
        assert_eq!(outside_said(&busy[2..]), "dumper pid 4000020 (1.0 cores)");
    }
}

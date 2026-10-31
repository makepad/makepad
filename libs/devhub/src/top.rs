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
    name: String,
    cpu_pct: f64,
    rss_mb: f64,
    gpu_mb: f64,
}

#[cfg(target_os = "linux")]
fn sample() -> (f64, Vec<ProcRow>) {
    use std::collections::HashMap;
    fn procs() -> HashMap<u32, (String, u64, u64)> {
        let mut out = HashMap::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
            let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
            let name = stat[a + 1..b].to_string();
            let f: Vec<&str> = stat[b + 2..].split_whitespace().collect();
            let num = |i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
            out.insert(pid, (name, num(11) + num(12), num(21) * 4096));
        }
        out
    }
    let read = || std::fs::read_to_string("/proc/stat").ok().and_then(|t| parse_proc_stat(&t));
    let (s0, p0) = (read(), procs());
    let t0 = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let (s1, p1) = (read(), procs());
    let secs = t0.elapsed().as_secs_f64().max(0.001);
    let tck = {
        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        let t = unsafe { sysconf(2) };
        if t > 0 { t as f64 } else { 100.0 }
    };
    let cpu = match (s0, s1) {
        (Some(a), Some(b)) => cpu_pct(a, b),
        _ => 0.0,
    };
    let rows = p1
        .into_iter()
        .map(|(pid, (name, ticks, rss))| {
            let before = p0.get(&pid).map_or(ticks, |p| p.1);
            ProcRow { pid, name, cpu_pct: (ticks.saturating_sub(before)) as f64 / tck / secs * 100.0, rss_mb: rss as f64 / 1048576.0, gpu_mb: 0.0 }
        })
        .collect();
    (cpu, rows)
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
            ProcRow { pid, name, cpu_pct: t.saturating_sub(before) as f64 / ns * 100.0, rss_mb: rss as f64 / 1048576.0, gpu_mb: 0.0 }
        })
        .collect();
    (cpu, rows)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn sample() -> (f64, Vec<ProcRow>) {
    // macOS (and the rest): `ps` gives a decaying CPU average per process; the machine's use is
    // their sum over the cores
    let out = makepad_remote::group::command("/bin/ps").args(["-Ao", "pid=,pcpu=,rss=,comm="]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let rows: Vec<ProcRow> = text
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let pid = w.next()?.parse().ok()?;
            let cpu = w.next()?.parse().ok()?;
            let rss: f64 = w.next()?.parse().ok()?;
            let comm = w.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            Some(ProcRow { pid, name, cpu_pct: cpu, rss_mb: rss / 1024.0, gpu_mb: 0.0 })
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
    rows.truncate(n);
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
        let g = parse_gpu_use("NVIDIA GeForce RTX 4090, 37, 4096, 24564, 51, 120.33\n");
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
}

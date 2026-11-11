//! Unix (Linux, macOS): a lease run is its own session (setsid), under `systemd-run --user --scope`
//! when there is a user bus, at its kind's nice level, optionally pinned with taskset; it is ended through
//! its process group.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

/// Exec limits measured on this node, also published in its caps.
pub fn exec_limits() -> Result<Option<super::ExecLimits>, String> {
    extern "C" {
        fn sysconf(name: i32) -> std::os::raw::c_long;
    }
    #[cfg(target_os = "macos")]
    const SC_ARG_MAX: i32 = 1;
    #[cfg(not(target_os = "macos"))]
    const SC_ARG_MAX: i32 = 0;
    // SAFETY: sysconf takes a constant selector and no pointers.
    let max = unsafe { sysconf(SC_ARG_MAX) };
    if max <= 0 { return Err("this box could not report its exec argument and env limit".into()); }
    let max = max as usize;
    #[cfg(target_os = "macos")]
    let one: Option<usize> = None;
    #[cfg(not(target_os = "macos"))]
    let one = {
        const SC_PAGESIZE: i32 = 30;
        // SAFETY: Linux's page-size selector takes no pointers.
        let page = unsafe { sysconf(SC_PAGESIZE) };
        if page <= 0 { return Err("this box could not report its exec page-size limit".into()); }
        Some(32 * page as usize)
    };
    Ok(Some(super::ExecLimits { total: max, per_string: one }))
}

/// Check an exec's words and explicit environment (the caller has cleared inheritance).
pub fn exec_over(cmd: &Command) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let limits = match exec_limits() { Ok(Some(limits)) => limits, Ok(None) => return None, Err(e) => return Some(e) };
    let pointer = std::mem::size_of::<*const u8>();
    let check = |label: String, bytes: usize| limits.word_over(&label, bytes, pointer);
    let program = Path::new(cmd.get_program()).file_name().unwrap_or(cmd.get_program()).to_string_lossy();
    let mut total = 2 * pointer; // the argv and env pointer arrays' terminating NULLs
    for (i, arg) in std::iter::once(cmd.get_program()).chain(cmd.get_args()).enumerate() {
        let bytes = arg.as_bytes().len() + 1;
        // (named by its start: its index is the wrapped command's, not the one the lane wrote)
        let start: String = arg.to_string_lossy().chars().take(40).collect();
        if let Some(over) = check(if i == 0 { format!("the program {program}") } else { format!("the argument `{start}...`") }, bytes) { return Some(over); }
        total += bytes + pointer;
    }
    for (key, value) in cmd.get_envs() {
        let Some(value) = value else { continue };
        let bytes = key.as_bytes().len() + 1 + value.as_bytes().len() + 1;
        if let Some(over) = check(format!("env value {}", key.to_string_lossy()), bytes) { return Some(over); }
        total += bytes + pointer;
    }
    limits.total_over(total)
}

/// What keeps a run's processes together beyond its pid: nothing on Unix (the process group is
/// named by the leader's pid).
#[derive(Default)]
pub struct Hold;

impl Hold {
    pub fn none() -> Hold {
        Hold
    }

    /// A run an earlier hub started: its group is its leader's pid, nothing to open.
    pub fn taken_back(_exit: &Path) -> Hold {
        Hold
    }
}

/// The words that take a run to nice `level` from this thread's: `nice` adds to what it starts from,
/// and `nice -n 10` under the hub the WM ran at 10 capped every run at 19, where a program's thread
/// classes are all one level (11-04). Nothing goes below this thread's level.
fn nice_words(level: i32) -> Option<[String; 3]> {
    let add = level - own_nice();
    (add > 0).then(|| ["nice".to_string(), "-n".to_string(), add.to_string()])
}

/// This thread's nice level.
pub fn own_nice() -> i32 {
    extern "C" {
        fn getpriority(which: i32, who: u32) -> i32;
    }
    // SAFETY: no pointers; who 0 is the calling thread (Linux levels are per thread).
    unsafe { getpriority(0, 0) }
}

/// The argv a lease run executes: the scope (collected even on failure), nice and pinning wrappers, then `argv`.
/// `weight`: a shared lease's scope CPU weight ([`crate::lease::Priority::cpu_weight`]): its
/// share of the box under contention, the whole box when idle. Never a CPU quota: a quota of the
/// lease's cores throttled a live Fab (measured at one core) to one CPU, and its pools and JIT
/// sized themselves to that one CPU.
/// `ram_max_gb`: the RAM the lease asked for itself (`--ram-gb`), the only hard memory cap; a
/// measured ask only schedules, never kills a run that needs more than it measured.
pub fn lease_argv(argv: Vec<String>, unit: Option<&str>, nice: i32, cpus: &str, weight: Option<u32>, ram_max_gb: Option<f64>) -> Vec<String> {
    let mut full: Vec<String> = Vec::new();
    if let Some(u) = unit {
        // the command's words stay its own: left to expand, systemd takes `$$` for `$` and `${X}`
        // for its value before the shell they are written for sees them
        full.extend(["systemd-run", "--user", "--scope", "--collect", "--quiet", "--expand-environment=no"].map(String::from));
        full.push(format!("--unit={u}"));
        if let Some(weight) = weight.filter(|_| cfg!(target_os = "linux")) {
            full.extend(["-p".into(), format!("CPUWeight={weight}")]);
            if let Some(gb) = ram_max_gb { full.extend(["-p".into(), format!("MemoryMax={}G", gb * 1.5)]); }
        }
        full.push("--".into());
    }
    full.extend(nice_words(nice).into_iter().flatten());
    if !cpus.is_empty() {
        full.extend(["taskset".to_string(), "-c".to_string(), cpus.to_string()]);
    }
    full.extend(argv);
    full
}

/// `program` as execvp finds it on `path`: itself when it names a path, else the first
/// executable file of that name in `path`'s directories. The hub finds a launch's wrapper so,
/// on its own PATH, under rustc's std and the JIT's alike ([`crate::lease::exec::Exec::launch`]).
pub fn hub_program(program: &str, path: &std::ffi::OsStr) -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    if program.contains('/') { return Ok(program.into()); }
    std::env::split_paths(path).filter(|d| !d.as_os_str().is_empty()).map(|d| d.join(program))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
        .ok_or_else(|| format!("{program}: not on the hub's PATH ({})", path.to_string_lossy()))
}

/// A run's scope (`unit`, Linux) takes CPU weight `weight` now: kept by systemd, a reload
/// leaves it so.
pub fn set_cpu_weight(unit: &str, weight: u32) -> Result<(), String> {
    let out = Command::new("systemctl").args(["--user", "set-property", "--runtime", &format!("{unit}.scope"), &format!("CPUWeight={weight}")])
        .stdin(std::process::Stdio::null()).output().map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("systemctl set-property {unit}.scope CPUWeight={weight}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// Error windows are Windows' alone ([`crate::os`]'s Windows `quiet_errors`).
pub fn quiet_errors() {}

/// Clear failed transient units left by earlier hubs; an unloaded unit is harmless.
pub fn reset_failed_units(pattern: &str) {
    let _ = Command::new("systemctl").args(["--user", "reset-failed", pattern])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

/// Every run goes under a small wrapper that writes the run's exit code beside its ledger
/// (`<id>-<n>.exit` beside `<id>-<n>.usage`) as it ends: a restarted hub (its self-update, at any moment) reads it, so no
/// run waits for a hub restart and no restart loses an exit. In a scope it also keeps the scope
/// alive until its final counters are saved, including runs shorter than a heartbeat. Arguments
/// are positional shell parameters, never interpolated shell source. The run's command goes first under a global OOM (Linux
/// `oom_score_adj` 500, the wrapper keeps its own): the kernel took the user manager (100) and the
/// session bus (200) before any lease process, and every service of the box died with them (11-03).
pub fn metered_argv(argv: Vec<String>, unit: Option<&str>, usage: &Path) -> Vec<String> {
    let script = r#"usage=$1
exitf=$2
scoped=$3
shift 3
group=
if [ "$scoped" = 1 ] && [ -r /proc/self/cgroup ]; then
    while IFS=: read -r hierarchy controllers path; do
        if [ "$hierarchy" = 0 ]; then group=/sys/fs/cgroup$path; fi
    done < /proc/self/cgroup
    printf '%s\n' "$group" > "$usage.cgroup"
fi
record() {
    if [ -n "$group" ]; then
        if [ -r "$group/cpu.stat" ]; then
            while read -r key value; do
                if [ "$key" = usage_usec ]; then printf 'cpu %s\n' "$value"; fi
            done < "$group/cpu.stat"
        fi
    fi
}
trap 'status=$?; record > "$usage"; printf "%s\n" "$status" > "$exitf.part"; mv -f "$exitf.part" "$exitf"' 0
trap 'exit 143' TERM
(
    if [ -w /proc/self/oom_score_adj ]; then printf '500\n' > /proc/self/oom_score_adj; fi
    exec "$@"
) &
child=$!
printf '%s\n' "$child" > "$usage.pid"
wait "$child"
exit "$?"
"#;
    let scoped = cfg!(target_os = "linux") && unit.is_some();
    let exit = usage.with_extension("exit");
    let mut full = vec!["sh".into(), "-c".into(), script.into(), "devhub-usage".into(), usage.to_string_lossy().into_owned(), exit.to_string_lossy().into_owned(), if scoped { "1" } else { "0" }.into()];
    full.extend(argv);
    full
}

/// What a scoped run holds now that its class's RAM is measured by ([`parse_unreclaimable`]), and
/// its cumulative CPU. The in-scope wrapper saves the CPU again at exit because systemd removes
/// an empty scope before its parent can read the cgroup.
pub fn scope_usage(pid: u32, start: u64, unit: Option<&str>, usage: &Path) -> (Option<u64>, Option<f64>) {
    let Some(group) = scope_dir(pid, start, unit, usage) else { return (None, None) };
    let read = |f: &str| std::fs::read_to_string(group.join(f)).ok();
    let peak = read("memory.current").and_then(|c| parse_unreclaimable(&c, &read("memory.stat").unwrap_or_default()));
    let cpu = std::fs::read_to_string(group.join("cpu.stat")).ok().and_then(|s| s.lines().find_map(|s| s.strip_prefix("usage_usec ").and_then(|s| s.parse::<f64>().ok()))).map(|us| us / 1_000_000.0);
    (peak, cpu)
}

/// A run's scope cgroup (Linux, a run under `systemd-run --scope`): the path its wrapper wrote,
/// else the live leader's own.
fn scope_dir(pid: u32, start: u64, unit: Option<&str>, usage: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") { return None; }
    let unit = unit?;
    let group = std::fs::read_to_string(usage.with_extension("usage.cgroup")).ok().map(|s| PathBuf::from(s.trim())).or_else(|| {
        if proc_start(pid) != Some(start) { return None; }
        std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|s| s.lines().find_map(|s| s.strip_prefix("0::").map(|s| Path::new("/sys/fs/cgroup").join(s.trim_start_matches('/')))))
    });
    group.filter(|g| g.starts_with("/sys/fs/cgroup") && g.file_name().is_some_and(|n| n == format!("{unit}.scope").as_str()))
}

/// What a run has done so far ([`super::Activity`]): a scoped run its scope's (`cpu.stat`, and
/// `io.stat`'s bytes), any other the processes of its group and their descendants; one measure
/// per run, as two are not comparable. None: nothing (yet, or left) to measure.
pub fn activity(pid: u32, start: u64, unit: Option<&str>, usage: &Path, _hold: &Hold) -> Option<super::Activity> {
    if cfg!(target_os = "linux") && unit.is_some() {
        let group = scope_dir(pid, start, unit, usage)?;
        let cpu = std::fs::read_to_string(group.join("cpu.stat")).ok()?.lines().find_map(|s| s.strip_prefix("usage_usec ").and_then(|s| s.trim().parse().ok()))?;
        let io = std::fs::read_to_string(group.join("io.stat")).unwrap_or_default().split_whitespace().filter_map(|w| w.strip_prefix("rbytes=").or_else(|| w.strip_prefix("wbytes="))).filter_map(|n| n.parse::<u64>().ok()).sum();
        return Some(super::Activity { cpu_us: cpu, io_bytes: io });
    }
    let members = group_members(&[(pid, start)])?;
    Some(members.into_iter().filter_map(proc_activity).fold(super::Activity::default(), |a, b| a + b))
}

/// One process's [`super::Activity`]: `stat`'s utime, stime, cutime and cstime (in the fixed
/// 100 Hz of `/proc`), and `io`'s read_bytes and write_bytes: storage I/O, as a scope's `io.stat`
/// counts it. A socket's or a pipe's traffic is none (rchar and wchar count it): a test polling its
/// parked app's remote every 50 ms moved 10-20 KB/s within its own processes and read as work (11-07).
#[cfg(target_os = "linux")]
fn proc_activity(pid: u32) -> Option<super::Activity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let f: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
    // f[11..15] = fields 14-17
    let ticks: u64 = f.get(11..15)?.iter().filter_map(|t| t.parse::<u64>().ok()).sum();
    let io = std::fs::read_to_string(format!("/proc/{pid}/io")).unwrap_or_default().lines().filter_map(|l| l.strip_prefix("read_bytes: ").or_else(|| l.strip_prefix("write_bytes: "))).filter_map(|n| n.trim().parse::<u64>().ok()).sum();
    Some(super::Activity { cpu_us: ticks * 10_000, io_bytes: io })
}

/// One process's [`super::Activity`] (`rusage_info_v2`): its own and its reaped children's CPU,
/// in Mach time units the timebase turns into ns, and its disk bytes.
#[cfg(target_os = "macos")]
fn proc_activity(pid: u32) -> Option<super::Activity> {
    /// `struct rusage_info_v2` (sys/resource.h).
    #[repr(C)]
    #[allow(dead_code)] // the C layout
    struct RusageInfoV2 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        proc_start_abstime: u64,
        proc_exit_abstime: u64,
        child_user_time: u64,
        child_system_time: u64,
        child_pkg_idle_wkups: u64,
        child_interrupt_wkups: u64,
        child_pageins: u64,
        child_elapsed_abstime: u64,
        diskio_bytesread: u64,
        diskio_byteswritten: u64,
    }
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut core::ffi::c_void) -> i32;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut r: RusageInfoV2 = unsafe { std::mem::zeroed() };
    // RUSAGE_INFO_V2 = 2
    if unsafe { proc_pid_rusage(pid as i32, 2, &mut r as *mut RusageInfoV2 as *mut _) } != 0 { return None; }
    let mut tb = Timebase { numer: 1, denom: 1 };
    unsafe { mach_timebase_info(&mut tb) };
    let ticks = (r.user_time + r.system_time + r.child_user_time + r.child_system_time) as u128;
    let ns = ticks * tb.numer as u128 / tb.denom.max(1) as u128;
    Some(super::Activity { cpu_us: (ns / 1000) as u64, io_bytes: r.diskio_bytesread + r.diskio_byteswritten })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn proc_activity(_pid: u32) -> Option<super::Activity> {
    None
}

/// This user's live processes in the groups `groups` leads or descended from one (a member may
/// move to a group of its own). A departed leader can still have members; a reused pid is
/// excluded. None: no group to look at.
fn group_members(groups: &[(u32, u64)]) -> Option<Vec<u32>> {
    let leaders: Vec<u32> = groups.iter().filter(|(pid, start)| *pid != 0 && proc_start(*pid).is_none_or(|s| s == *start)).map(|(pid, _)| *pid).collect();
    if leaders.is_empty() {
        return None;
    }
    // pid -> (parent, in one of the groups)
    let table = process_table(&leaders);
    // its own group or an ancestor's; the walk is bounded, as one look at the table is no
    // consistent snapshot
    let member = |pid: u32| {
        let mut p = pid;
        for _ in 0..64 {
            match table.get(&p) {
                Some((_, true)) => return true,
                Some((parent, _)) if *parent != p => p = *parent,
                _ => return false,
            }
        }
        false
    };
    Some(table.keys().copied().filter(|pid| member(*pid)).collect())
}

/// The processes of one run: its group and what descended from it.
pub fn group_pids(pid: u32, start: u64, _hold: &Hold) -> Vec<u32> {
    group_members(&[(pid, start)]).unwrap_or_default()
}

/// What `pids` do now, for someone reading a stuck run: each one's command, its threads by name,
/// state and kernel wait, then gdb's stacks of those with no child among them, or why it gave none.
#[cfg(target_os = "linux")]
pub fn processes_view(pids: &[u32]) -> String {
    let mut out = String::new();
    let mut parents = Vec::new();
    for &pid in pids {
        let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        // (a run's wrapper is a whole script: its first line says enough)
        let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ");
        let cmd: String = cmd.trim().lines().next().unwrap_or("").chars().take(200).collect();
        out.push_str(&format!("pid {pid}: {cmd}\n"));
        let mut alike = std::collections::BTreeMap::<(String, String, String), usize>::new();
        for t in std::fs::read_dir(format!("/proc/{pid}/task")).into_iter().flatten().flatten() {
            let Ok(stat) = std::fs::read_to_string(t.path().join("stat")) else { continue };
            let Some((name, rest)) = stat.split_once('(').and_then(|(_, r)| r.rsplit_once(')')) else { continue };
            let f: Vec<&str> = rest.split_whitespace().collect();
            if t.file_name().to_string_lossy() == pid.to_string() { parents.extend(f.get(1).and_then(|p| p.parse::<u32>().ok())); }
            let state = f.first().copied().unwrap_or("?");
            // (`0`: on a CPU, or closed to this process: a JIT is not dumpable)
            let wait = std::fs::read_to_string(t.path().join("wchan")).unwrap_or_default();
            let wait = match wait.trim() {
                "" | "0" if state == "R" => "on cpu",
                "" | "0" => "?",
                w => w,
            };
            *alike.entry((name.to_string(), state.to_string(), wait.to_string())).or_default() += 1;
        }
        for ((name, state, wait), n) in alike {
            out.push_str(&format!("  {n}x {name} {state} {wait}\n"));
        }
    }
    for &pid in pids.iter().filter(|p| !parents.contains(p)) {
        match stacks(pid) {
            Ok(frames) => out.push_str(&format!("stacks of {pid}:\n{frames}\n")),
            // one gone meanwhile says nothing of the box: a poll loop's `sleep 0.1` ended under gdb
            // and the hung stub after it went without its stacks (11-08)
            Err(_) if !Path::new(&format!("/proc/{pid}")).exists() => {}
            // it is the box's answer, the same for the next one
            Err(why) => { out.push_str(&format!("stacks of {pid}: {why}\n")); break; }
        }
    }
    out
}

/// Where a stuck run of `pids` waits: the oldest of them that started none of the others (what the
/// rest wait on), as the chain of commands that leads to it from the first of them, collapsed
/// where a shell's subshell repeats it, with its pid and how long it has run, in s. A jitlegs
/// leg's hung fixture stub slept under five shells, beside a poll loop's ever-new `sleep 0.1`
/// (11-08). None: none of them lives.
#[cfg(target_os = "linux")]
pub fn stuck_in(pids: &[u32]) -> Option<(String, u32, u64)> {
    // /proc's start times are in its fixed 100 Hz
    let up_s: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    // pid -> (parent, start), of the live ones (a zombie does nothing)
    let procs: std::collections::BTreeMap<u32, (u32, u64)> = pids.iter().filter_map(|&pid| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let f: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
        if f.first() == Some(&"Z") { return None; }
        Some((pid, (f.get(1)?.parse().ok()?, f.get(19)?.parse().ok()?)))
    }).collect();
    let (&leaf, &(_, start)) = procs.iter().filter(|(p, _)| !procs.values().any(|(parent, _)| parent == *p)).min_by_key(|(p, (_, start))| (*start, **p))?;
    let mut chain: Vec<String> = Vec::new();
    let mut at = Some(leaf);
    for _ in 0..procs.len() {
        let Some(p) = at else { break };
        if let Some(label) = crate::top::label(p).filter(|l| chain.last() != Some(l)) { chain.push(label); }
        at = procs.get(&p).map(|(parent, _)| *parent).filter(|parent| procs.contains_key(parent));
    }
    chain.reverse();
    Some((chain.join(" > "), leaf, (up_s - start as f64 / 100.0).max(0.0) as u64))
}

/// (No process table read here.)
#[cfg(not(target_os = "linux"))]
pub fn stuck_in(_pids: &[u32]) -> Option<(String, u32, u64)> {
    None
}

/// gdb's stacks of a process. Yama's default lets a process trace only its own descendants, so on
/// such a box only the user's `sysctl kernel.yama.ptrace_scope=0` opens this (and nothing does for a
/// process that is not dumpable).
#[cfg(target_os = "linux")]
fn stacks(pid: u32) -> Result<String, String> {
    let o = std::process::Command::new("gdb").args(["-q", "-n", "-batch", "-p", &pid.to_string(), "-ex", "thread apply all bt 20"]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("no gdb here: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    let frames: Vec<&str> = text.lines().filter(|l| l.starts_with("Thread ") || l.starts_with('#')).collect();
    if !frames.is_empty() { return Ok(frames.join("\n")); }
    let scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope").unwrap_or_default();
    Err(format!("{} (kernel.yama.ptrace_scope={})", text.lines().find(|l| l.contains("ptrace:")).unwrap_or("gdb gave none"), scope.trim()))
}

#[cfg(not(target_os = "linux"))]
pub fn processes_view(pids: &[u32]) -> String {
    pids.iter().map(|p| format!("pid {p}\n")).collect()
}

/// pid -> (parent, in one of the `leaders`' groups), of this user's live processes.
#[cfg(target_os = "macos")]
fn process_table(leaders: &[u32]) -> std::collections::HashMap<u32, (u32, bool)> {
    extern "C" {
        fn proc_listallpids(buffer: *mut core::ffi::c_void, size: i32) -> i32;
        fn getuid() -> u32;
    }
    let mut pids = vec![0i32; 8192];
    let n = unsafe { proc_listallpids(pids.as_mut_ptr() as *mut _, (pids.len() * 4) as i32) };
    let uid = unsafe { getuid() };
    let mut table = std::collections::HashMap::new();
    for pid in pids.iter().take(n.max(0) as usize).filter(|p| **p > 0).map(|p| *p as u32) {
        if let Some(b) = bsd_info(pid).filter(|b| b.uid == uid && b.status != 5) {
            table.insert(pid, (b.ppid, leaders.contains(&b.pgid)));
        }
    }
    table
}

/// pid -> (parent, in one of the `leaders`' groups), of the live processes `/proc` shows.
#[cfg(target_os = "linux")]
fn process_table(leaders: &[u32]) -> std::collections::HashMap<u32, (u32, bool)> {
    let mut table = std::collections::HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        let Some(at) = stat.rfind(')') else { continue };
        let f: Vec<&str> = stat[at + 2..].split_whitespace().collect();
        // f[0] = state, f[1] = ppid, f[2] = pgrp
        let (Some(ppid), Some(pgrp)) = (f.get(1).and_then(|p| p.parse::<u32>().ok()), f.get(2).and_then(|p| p.parse::<u32>().ok())) else { continue };
        if f.first() != Some(&"Z") { table.insert(pid, (ppid, leaders.contains(&pgrp))); }
    }
    table
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_table(_leaders: &[u32]) -> std::collections::HashMap<u32, (u32, bool)> {
    std::collections::HashMap::new()
}

/// What a scoped run holds now that the kernel cannot reclaim, bytes: its scope's anonymous and
/// shared memory (`memory.stat`), not its page cache. None: no scope (or it is gone).
pub fn scope_now(pid: u32, start: u64, unit: Option<&str>, usage: &Path) -> Option<u64> {
    let stat = std::fs::read_to_string(scope_dir(pid, start, unit, usage)?.join("memory.stat")).ok()?;
    Some(parse_held(&stat))
}

/// A cgroup's `memory.current` less the page cache the kernel reclaims first (`memory.stat`'s
/// `inactive_file`), bytes: what a run's class asks for. Never `memory.peak`, which counts every
/// file the run read: a third of a tests shard's scope was idle cache, and its class asked
/// 27.7 GB and waited 15 min for a box with that free (11-10).
pub fn parse_unreclaimable(current: &str, stat: &str) -> Option<u64> {
    let current: u64 = current.trim().parse().ok()?;
    let idle = stat.lines().find_map(|l| l.strip_prefix("inactive_file ")).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
    Some(current.saturating_sub(idle))
}

/// `anon` + `shmem` of a cgroup's `memory.stat`.
pub fn parse_held(stat: &str) -> u64 {
    stat.lines().filter_map(|l| l.split_once(' ')).filter(|(k, _)| matches!(*k, "anon" | "shmem")).filter_map(|(_, v)| v.trim().parse::<u64>().ok()).sum()
}

/// Whether [`groups_held`] measures a run's processes (a run's memory can be told from the box's).
pub const MEASURES_GROUPS: bool = true;

/// What the runs whose process groups `groups` leads hold, bytes: their resident set.
#[cfg(not(target_os = "macos"))]
pub fn groups_held(groups: &[(u32, u64)]) -> Option<u64> {
    groups_rss(groups)
}

/// RSS in bytes across recorded process groups (ps reports KiB), in one process-table
/// snapshot per lease. A departed leader can still have children; a reused pid is excluded.
pub fn groups_rss(groups: &[(u32, u64)]) -> Option<u64> {
    let groups: Vec<_> = groups.iter().filter(|(pid, start)| *pid != 0 && proc_start(*pid).is_none_or(|s| s == *start)).map(|(pid, _)| pid.to_string()).collect();
    if groups.is_empty() { return None; }
    let out = Command::new("ps").args(["-o", "rss=", "-g", &groups.join(",")]).output().ok()?;
    if !out.status.success() { return None; }
    let rss: Vec<u64> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
    (!rss.is_empty()).then(|| rss.iter().sum::<u64>() * 1024)
}

/// Reap just this child, retaining user + system CPU including its waited-for descendants, and
/// the largest resident set (bytes) one of them reached: the one measure of a run that ended
/// before anything looked at it. `Child::wait` discards rusage; a process-wide RUSAGE_CHILDREN
/// delta would mix leases.
pub fn wait_usage(child: &mut Child) -> std::io::Result<(ExitStatus, Option<f64>, Option<u64>)> {
    loop {
        if let Some(reaped) = wait4_usage(child, 0)? { return Ok(reaped); }
    }
}

/// [`wait_usage`] without waiting: None while the child runs (`Child::try_wait` discards rusage too).
pub fn try_wait_usage(child: &mut Child) -> std::io::Result<Option<(ExitStatus, Option<f64>, Option<u64>)>> {
    const WNOHANG: i32 = 1;
    wait4_usage(child, WNOHANG)
}

/// One `wait4` of the child with `options`: None when it has not ended (WNOHANG) or the wait was
/// interrupted.
fn wait4_usage(child: &mut Child, options: i32) -> std::io::Result<Option<(ExitStatus, Option<f64>, Option<u64>)>> {
    use std::os::raw::c_long;
    use std::os::unix::process::ExitStatusExt;
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        sec: c_long,
        #[cfg(target_os = "macos")]
        usec: i32,
        #[cfg(not(target_os = "macos"))]
        usec: c_long,
    }
    #[repr(C)]
    #[derive(Default)]
    struct RUsage { user: Timeval, system: Timeval, rest: [c_long; 14] }
    extern "C" { fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut RUsage) -> i32; }
    let mut status = 0;
    let mut usage = RUsage::default();
    match unsafe { wait4(child.id() as i32, &mut status, options, &mut usage) } {
        0 => Ok(None),
        n if n > 0 => {
            let cpu = (usage.user.sec + usage.system.sec) as f64 + (usage.user.usec + usage.system.usec) as f64 / 1_000_000.0;
            // ru_maxrss: bytes on macOS, KiB on Linux
            let maxrss = usage.rest[0].max(0) as u64 * if cfg!(target_os = "macos") { 1 } else { 1024 };
            Ok(Some((ExitStatus::from_raw(status), Some(cpu), (maxrss > 0).then_some(maxrss))))
        }
        _ => {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted { Ok(None) } else { Err(e) }
        }
    }
}

/// Starts a lease run in its own session and process group (the group is what end signals; `exit`
/// names the run's job on Windows).
pub fn lease_spawn(cmd: &mut Command, _exit: Option<&Path>) -> std::io::Result<(Child, Hold)> {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    Ok((cmd.spawn()?, Hold))
}

/// The exit code, 128 + the signal for a run a signal ended.
pub fn exit_code(s: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))
}

/// Whether `pid` lives (signal 0; EPERM still means it lives).
pub fn pid_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1) }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `program` as a background build. On macOS at background QoS: efficiency cores and throttled
/// I/O, a clamp cargo's rustc inherit and cannot lift, so a build never competes with the apps the
/// person works in (nice only orders it behind them: after a landing a nice-10 build beside another
/// took rustc to ~1000% of a core and the person's Mac to a load of 45, 11-05). It only takes
/// longer. Elsewhere at nice 15.
pub fn background(program: &std::ffi::OsStr) -> Command {
    let (low, words): (&str, &[&str]) = if cfg!(target_os = "macos") { ("/usr/sbin/taskpolicy", &["-c", "background"]) } else { ("nice", &["-n", "15"]) };
    let mut cmd = Command::new(low);
    cmd.args(words).arg(program);
    cmd
}

/// The login name.
pub fn login() -> Option<String> {
    std::env::var("USER").ok()
}

/// (allocated bytes, mtime in s) of one directory entry.
pub fn on_disk(m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.blocks() * 512, m.mtime().max(0) as u64)
}

/// A process's start time (Linux: clock ticks since boot; macOS: µs since the epoch).
pub fn proc_start(pid: u32) -> Option<u64> {
    proc_stat(pid).map(|s| s.start)
}

struct Stat {
    start: u64,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pgrp: u32,
    zombie: bool,
}

#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let f: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
    // f[0] = field 3 (state), f[2] = 5 (pgrp), f[19] = 22 (starttime)
    Some(Stat { start: f.get(19)?.parse().ok()?, pgrp: f.get(2)?.parse().ok()?, zombie: f.first() == Some(&"Z") })
}

/// `struct proc_bsdinfo` (sys/proc_info.h).
#[cfg(target_os = "macos")]
#[repr(C)]
#[allow(dead_code)] // the C layout: every field is there so the kernel's writes land in place
struct BsdInfo {
    flags: u32,
    status: u32,
    xstatus: u32,
    pid: u32,
    ppid: u32,
    uid: u32,
    gid: u32,
    ruid: u32,
    rgid: u32,
    svuid: u32,
    svgid: u32,
    rfu: u32,
    comm: [u8; 16],
    name: [u8; 32],
    nfiles: u32,
    pgid: u32,
    pjobc: u32,
    tdev: u32,
    tpgid: u32,
    nice: i32,
    start_sec: u64,
    start_usec: u64,
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<BsdInfo> {
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut core::ffi::c_void, size: i32) -> i32;
    }
    let mut b: BsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<BsdInfo>() as i32;
    // PROC_PIDTBSDINFO = 3
    (unsafe { proc_pidinfo(pid as i32, 3, 0, &mut b as *mut BsdInfo as *mut _, size) } == size).then_some(b)
}

#[cfg(target_os = "macos")]
fn proc_stat(pid: u32) -> Option<Stat> {
    let b = bsd_info(pid)?;
    // SZOMB = 5
    Some(Stat { start: b.start_sec * 1_000_000 + b.start_usec, pgrp: b.pgid, zombie: b.status == 5 })
}

/// A process's physical footprint, bytes (`phys_footprint`, Activity Monitor's Memory column):
/// its dirty memory, compressed or not, and its GPU memory; not its file cache, nor purgeable or
/// reusable (`MADV_FREE_REUSABLE`) pages the kernel takes back at will.
#[cfg(target_os = "macos")]
fn footprint(pid: u32) -> Option<u64> {
    /// `struct rusage_info_v0` (sys/resource.h).
    #[repr(C)]
    #[allow(dead_code)] // the C layout
    struct RusageInfoV0 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        proc_start_abstime: u64,
        proc_exit_abstime: u64,
    }
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut core::ffi::c_void) -> i32;
    }
    let mut r: RusageInfoV0 = unsafe { std::mem::zeroed() };
    // RUSAGE_INFO_V0 = 0
    (unsafe { proc_pid_rusage(pid as i32, 0, &mut r as *mut RusageInfoV0 as *mut _) } == 0).then_some(r.phys_footprint)
}

/// What the runs whose process groups `groups` leads hold, bytes: the footprints ([`footprint`])
/// of this user's processes in those groups or descended from one (a member may move to a group
/// of its own). A departed leader can still have members; a reused pid is excluded. Their
/// resident set counted the file pages they map (their binaries), which the kernel's in-use
/// count ([`crate::procinfo::in_use`]) leaves out. None: no group to measure.
#[cfg(target_os = "macos")]
pub fn groups_held(groups: &[(u32, u64)]) -> Option<u64> {
    Some(group_members(groups)?.into_iter().filter_map(footprint).sum())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn proc_stat(_pid: u32) -> Option<Stat> {
    None
}

/// Whether `pid` has ended and waits to be reaped (a zombie answers `kill(pid, 0)` as alive).
pub fn zombie(pid: u32) -> bool {
    proc_stat(pid).is_some_and(|s| s.zombie)
}

/// Whether `pid` is in the process group led by `leader` (a lease run is its own session).
pub fn in_group(pid: u32, leader: u32, _hold: &Hold) -> bool {
    proc_stat(pid).is_some_and(|s| s.pgrp == leader && !s.zombie)
}

/// The recorded leader still is the process we started (not gone, not a reused pid).
pub fn same(pid: u32, start: u64) -> bool {
    proc_stat(pid).is_some_and(|s| !s.zombie && s.start == start)
}

/// Whether the run's process group still has a live member. Its pgid is the leader's pid,
/// which the OS does not hand out again while the group exists; a reused leader pid (a
/// different start time) means the group is gone.
pub fn group_alive(pid: u32, start: u64, _hold: &Hold) -> bool {
    if let Some(s) = proc_stat(pid) {
        if s.start != start {
            return false;
        }
        if !s.zombie {
            return true;
        }
    }
    #[cfg(target_os = "linux")]
    {
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            if let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() {
                if p != pid && proc_stat(p).is_some_and(|s| s.pgrp == pid && !s.zombie) {
                    return true;
                }
            }
        }
    }
    // the members too, as on Linux: a run's leader is the usage wrapper's `sh`, which a TERM ends
    // at once while the program it waits for winds down. Asking only the leader, a lease's end
    // took the group for gone, never KILLed it, and found the JIT app it ran as a process "that
    // had left its process groups" (a show pre-boot's Fab on a Mac, 11-03)
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn proc_listpids(kind: u32, typeinfo: u32, buffer: *mut core::ffi::c_void, size: i32) -> i32;
        }
        let mut pids = vec![0i32; 4096];
        // PROC_PGRP_ONLY = 2; the answer is in bytes
        let n = unsafe { proc_listpids(2, pid, pids.as_mut_ptr() as *mut _, (pids.len() * 4) as i32) };
        if pids.iter().take(n.max(0) as usize / 4).any(|&p| p > 0 && p as u32 != pid && proc_stat(p as u32).is_some_and(|s| s.pgrp == pid && !s.zombie)) {
            return true;
        }
    }
    false
}

/// TERM (`hard`: KILL) one process.
pub fn signal_one(pid: u32, hard: bool) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(pid as i32, if hard { 9 } else { 15 });
    }
}

/// The live processes of this user whose environment has `marker` (`NAME=value`), whatever session
/// or process group they moved to.
#[cfg(target_os = "linux")]
pub fn with_env(marker: &str) -> Vec<u32> {
    env_values(marker).into_iter().filter(|(_, v)| v.is_empty()).map(|(p, _)| p).collect()
}

/// The live processes of this user with an environment entry starting `prefix` (`NAME=`), and
/// what follows it.
#[cfg(target_os = "linux")]
pub fn env_values(prefix: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if let Ok(env) = std::fs::read(e.path().join("environ")) {
            if let Some(v) = env.split(|b| *b == 0).find_map(|v| v.strip_prefix(prefix.as_bytes())) {
                out.push((pid, String::from_utf8_lossy(v).into_owned()));
            }
        }
    }
    out
}

/// As on Linux: the environment comes with a process's arguments (KERN_PROCARGS2).
#[cfg(target_os = "macos")]
pub fn with_env(marker: &str) -> Vec<u32> {
    env_values(marker).into_iter().filter(|(_, v)| v.is_empty()).map(|(p, _)| p).collect()
}

#[cfg(target_os = "macos")]
pub fn env_values(prefix: &str) -> Vec<(u32, String)> {
    extern "C" {
        fn proc_listallpids(buffer: *mut core::ffi::c_void, size: i32) -> i32;
        fn sysctl(name: *const i32, namelen: u32, oldp: *mut u8, oldlenp: *mut usize, newp: *const u8, newlen: usize) -> i32;
    }
    const CTL_KERN: i32 = 1;
    const KERN_PROCARGS2: i32 = 49;
    let mut pids = vec![0i32; 8192];
    let n = unsafe { proc_listallpids(pids.as_mut_ptr() as *mut _, (pids.len() * 4) as i32) };
    let mut buf = vec![0u8; 1 << 20];
    let mut out = Vec::new();
    for &pid in pids.iter().take(n.max(0) as usize).filter(|p| **p > 0) {
        let mib = [CTL_KERN, KERN_PROCARGS2, pid];
        let mut len = buf.len();
        if unsafe { sysctl(mib.as_ptr(), 3, buf.as_mut_ptr(), &mut len, std::ptr::null(), 0) } == 0 {
            if let Some(v) = buf[..len].split(|b| *b == 0).find_map(|v| v.strip_prefix(prefix.as_bytes())) {
                out.push((pid as u32, String::from_utf8_lossy(v).into_owned()));
            }
        }
    }
    out
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn with_env(_marker: &str) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn env_values(_prefix: &str) -> Vec<(u32, String)> {
    Vec::new()
}

/// TERM (`hard`: KILL) the run's process group.
pub fn signal_group(pid: u32, hard: bool, _hold: &Hold) {
    makepad_remote::group::signal(pid, hard);
}

/// A lease's end asks the run to exit: TERM to its group; whether the group got it.
pub fn term_group(pid: u32, _hold: &Hold) -> bool {
    makepad_remote::group::signal(pid, false)
}


/// The measuring walk runs at nice 10 (Linux: per thread). A Mac's level is the whole process's:
/// its thread takes the utility class there, behind the person's apps.
pub fn nice_this_thread() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        // SAFETY: no pointers; it changes the calling thread only.
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
            fn gettid() -> i32;
        }
        unsafe {
            setpriority(0, gettid() as u32, 10);
        }
    }
}


/// The descriptors this process got from its parent stay its own: every one above stdio is
/// close-on-exec from here on, so a run gets only the descriptors it is given (its stdio), never
/// the hub's. A hub under the WM had three of the WM's GPU buffers (dmabuf) open without
/// close-on-exec, and every run inherited them, a run that outlived its hub too (11-04). Called
/// at the hub's start, before it starts anything; what Rust opens is close-on-exec already.
pub fn keep_handles_to_self() {
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    // the same values on Linux and macOS
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    let listing = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    // (the listing's own descriptor is among them: close-on-exec already)
    let fds: Vec<i32> = std::fs::read_dir(listing).map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).filter(|&fd| fd > 2).collect()).unwrap_or_default();
    for fd in fds {
        let flags = unsafe { fcntl(fd, F_GETFD) };
        if flags >= 0 && flags & FD_CLOEXEC == 0 {
            unsafe { fcntl(fd, F_SETFD, flags | FD_CLOEXEC) };
        }
    }
}

/// Raises the soft open-files limit to the hard one (macOS caps it at 10240), as hygiene: a hub
/// holds a descriptor or two per link and per lease, and a 256 soft limit is a small ceiling.
/// Returns the soft limit now.
pub fn raise_fd_limit() -> u64 {
    #[repr(C)]
    struct RLimit {
        cur: u64,
        max: u64,
    }
    extern "C" {
        fn getrlimit(resource: i32, rlim: *mut RLimit) -> i32;
        fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
    }
    #[cfg(target_os = "linux")]
    const RLIMIT_NOFILE: i32 = 7;
    #[cfg(not(target_os = "linux"))]
    const RLIMIT_NOFILE: i32 = 8;
    let mut r = RLimit { cur: 0, max: 0 };
    if unsafe { getrlimit(RLIMIT_NOFILE, &mut r) } != 0 {
        return 0;
    }
    #[cfg(target_os = "macos")]
    let want = r.max.min(10240);
    #[cfg(not(target_os = "macos"))]
    let want = r.max;
    if want > r.cur {
        let n = RLimit { cur: want, max: r.max };
        unsafe { setrlimit(RLIMIT_NOFILE, &n) };
    }
    if unsafe { getrlimit(RLIMIT_NOFILE, &mut r) } != 0 { return 0; }
    r.cur
}

/// (a Unix run's wrapper is a shell script: see the Windows one)
#[cfg(test)]
pub fn no_image_roles() -> bool {
    false
}

/// A run's level is the one asked for, whatever level its hub runs at.
#[cfg(all(test, target_os = "linux"))]
#[test]
fn a_runs_nice_does_not_stack_on_its_hubs() {
    let ni = |level: i32| {
        let argv = lease_argv(["sh", "-c", "ps -o ni= -p $$"].map(String::from).to_vec(), None, level, "", None, None);
        let out = Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<i32>().unwrap()
    };
    let own = ni(-20);
    let level = (own + 3).min(19);
    assert_eq!(ni(level), level, "from {own}");
    assert_eq!(ni(own - 1), own, "nothing goes below the hub's level");
}

/// A run's words reach it as written: a shell's `$$` and `${X}` are the shell's.
#[cfg(all(test, target_os = "linux"))]
#[test]
fn a_scope_leaves_a_runs_dollars_to_its_shell() {
    let argv = lease_argv(["sh", "-c", "echo $$ ${X}y"].map(String::from).to_vec(), Some("devhub-test-dollars"), 10, "", None, None);
    assert!(argv.iter().any(|a| a == "--expand-environment=no"), "{argv:?}");
    if !crate::lease::exec::user_bus() { return; }
    let unit = format!("devhub-test-dollars-{}", std::process::id());
    let argv = lease_argv(["sh", "-c", "echo $$ ${X}y"].map(String::from).to_vec(), Some(&unit), 10, "", None, None);
    let out = Command::new(&argv[0]).args(&argv[1..]).env("X", "from-the-run").output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    let (pid, x) = out.trim().split_once(' ').unwrap_or_else(|| panic!("{out}"));
    assert!(pid.parse::<u32>().is_ok(), "the shell's own pid, not a lone dollar: {out}");
    assert_eq!(x, "from-the-runy");
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn safety_raised_limit_is_the_open_file_limit() {
    let actual = raise_fd_limit();
    let limits = std::fs::read_to_string("/proc/self/limits").unwrap();
    let soft = limits.lines().find_map(|l| l.strip_prefix("Max open files")).unwrap()
        .split_whitespace().next().unwrap().parse::<u64>().unwrap();
    assert_eq!(actual, soft);
}

/// A scope's class RAM is what it holds less its idle page cache, from a tests shard's numbers
/// (11.6 GB held, 3.5 GB of it idle cache); a cgroup without the stat line keeps it all.
#[cfg(test)]
#[test]
fn a_scopes_class_ram_leaves_out_its_idle_page_cache() {
    let mb = 1u64 << 20;
    let stat = format!("anon {}\nfile {}\nkernel {}\nshmem {}\nactive_file {}\ninactive_file {}\n", 7953 * mb, 3798 * mb, 53 * mb, 89 * mb, 87 * mb, 3621 * mb);
    assert_eq!(parse_unreclaimable(&format!("{}\n", 11888 * mb), &stat), Some(8267 * mb));
    assert_eq!(parse_unreclaimable("4096\n", "anon 4096\n"), Some(4096), "no inactive_file: all of it");
    assert_eq!(parse_unreclaimable("", &stat), None, "no memory.current: no measure");
}

/// A run's group and what descends from it count, by footprint; this process does not.
#[cfg(all(test, target_os = "macos"))]
#[test]
fn groups_held_counts_a_groups_members_and_their_descendants() {
    use std::os::unix::process::CommandExt;
    // a run's leader in its own group with two members; another run's group of one
    let mut run = Command::new("sh").args(["-c", "sleep 30 & sleep 30; wait"]).process_group(0).spawn().unwrap();
    let mut other = Command::new("sleep").arg("30").process_group(0).spawn().unwrap();
    let start = proc_start(run.id()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let looks = [groups_held(&[(run.id(), start)]), groups_held(&[(other.id(), proc_start(other.id()).unwrap())]), groups_held(&[(run.id(), start + 1)]), groups_held(&[])];
    for c in [&mut run, &mut other] {
        signal_group(c.id(), true, &Hold);
        let _ = c.wait();
    }
    let [run_held, other_held, reused, none] = looks;
    assert!(run_held.unwrap() > other_held.unwrap() && other_held.unwrap() > 0, "{looks:?}");
    assert_eq!((reused, none), (None, None), "{looks:?}");
}

/// A descriptor the hub got from its parent without close-on-exec (the WM's GPU buffers) reaches
/// no run once the hub keeps its handles to itself; before, every run held it.
#[test]
fn a_run_gets_none_of_the_hubs_descriptors() {
    use std::os::fd::AsRawFd;
    extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    let dir = std::env::temp_dir().join(format!("devhub-descriptors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join("passed")).unwrap();
    let fd = file.as_raw_fd();
    // as a parent passes it: not close-on-exec (F_SETFD, no flags)
    assert_eq!(unsafe { fcntl(fd, 2, 0) }, 0);
    let in_run = || {
        // no redirection in the probe: a shell saves stderr at the first free descriptor from 10
        // on, so `{ true >&10; } 2>/dev/null` found its own copy when the file was 10 (11-04)
        let out = Command::new("sh").arg("-c").arg(format!("if [ -e /dev/fd/{fd} ]; then echo held; else echo free; fi")).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(in_run(), "held", "a run sees a descriptor passed to it");
    keep_handles_to_self();
    assert_eq!(in_run(), "free");
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
}

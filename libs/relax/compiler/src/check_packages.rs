//! `makepad check -p a -p b ...`: each package checked by a process of its own, as `makepad check
//! <package>` checks it, several side by side. Each child's stdout and stderr are kept until the
//! packages before it in `-p` order are written: per stream, the command prints what the children
//! printed one after another. The exit is the worst child's.
//!
//! How many at once: the cores the command was given (MAKEPAD_CPU_CORES, a lease's share; else
//! the machine's) over the threads a child gets (`CHILD_JOBS`), lowered while the children's
//! measured peak memory (the p95 of those finished) times that many is over half the machine's
//! RAM. A single child at a time is the serial run, its output not held.
//!
//! They start in `-p` order. Each is checked as alone: the children share only the stores, as
//! two checks of the same machine do. They are one pass (`RELAX_CHECK_GROUP`, this command's
//! pid): the command holds the pass from the first child to the last, so no store a child
//! touched is evicted while a later one runs (store.rs passes, cache.rs prune). Two that need the
//! same build script's output wait on its claim, one running it; such a wait on a sibling is kept
//! beside the claim (relax-claim-waits) and on stderr only with RELAX_PROFILE: the serial run,
//! which never waits, prints none.
use std::path::Path;
use std::process::{Command, Stdio};

/// The threads a child check gets (RELAX_JOBS and its CPU budget, MAKEPAD_CPU_CORES): a check is
/// mostly one thread (load, collection) with a short burst on its jobs in the bodies, about 1.5
/// cores a process over a gate's dependents.
const CHILD_JOBS: usize = 2;

/// One `-p` package: its name, and the arguments of its check (`package_args`).
pub(crate) struct Run {
    pub package: String,
    pub args: Vec<String>,
}

/// `-p <package>`: a package like one named plainly. Several: each checked by a process of its
/// own (module doc); Err is the command's exit. Ok: the arguments of the one check, here.
pub(crate) fn check_packages(args: &[String], grants: &[String]) -> Result<Vec<String>, i32> {
    let mut runs = package_args(args)?;
    if runs.len() < 2 {
        return Ok(runs.remove(0).args);
    }
    let exe = crate::dirs::loader_exe().map_err(|e| {
        eprintln!("relax check: the loader's own path: {e}");
        2
    })?;
    crate::store::hold_pass(&crate::dirs::cache_dir());
    let width = (makepad_cpu_budget::cores() as usize / CHILD_JOBS).max(1);
    if width == 1 {
        return Err(serial(&exe, &runs, grants));
    }
    Err(side_by_side(&exe, &runs, grants, width))
}

/// Each package's check in turn, its output the terminal's.
fn serial(exe: &Path, runs: &[Run], grants: &[String]) -> i32 {
    let mut worst = 0;
    for r in runs {
        let status = Command::new(exe).arg("check").args(grants).args(&r.args).env("RELAX_CHECK_GROUP", crate::store::pass_token()).status();
        let code = status.ok().and_then(|s| s.code()).unwrap_or(2);
        if code != 0 {
            eprintln!("relax check: {}: exit {code}", r.package);
        }
        worst = worst.max(code);
    }
    worst
}

/// A finished child: its output, exit and peak resident memory (bytes; 0: not known).
struct Done {
    index: usize,
    out: Vec<u8>,
    err: Vec<u8>,
    code: i32,
    peak: u64,
}

/// The checks of `runs`, up to `width` at once, started and written in `-p` order.
fn side_by_side(exe: &Path, runs: &[Run], grants: &[String], width: usize) -> i32 {
    // the children's output, in files: a child that fills one pipe while its reader waits on
    // the other would stop
    let dir = crate::dirs::relax_dir().join("check-packages");
    if let Err(e) = crate::pack::ensure_private_dir(&dir) {
        eprintln!("relax check: {}: {e}; checking one package at a time", dir.display());
        return serial(exe, runs, grants);
    }
    let ram = crate::stack::physical_memory() as u64;
    let (work_tx, work_rx) = std::sync::mpsc::channel::<usize>();
    let work_rx = std::sync::Mutex::new(work_rx);
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    let mut worst = 0;
    std::thread::scope(|scope| {
        // one worker per child at once, each starting a child and waiting for it
        for w in 0..width {
            let (work_rx, done_tx, dir) = (&work_rx, done_tx.clone(), &dir);
            std::thread::Builder::new().name(format!("check-package-{w}")).spawn_scoped(scope, move || loop {
                let Ok(i) = work_rx.lock().unwrap().recv() else { return };
                let _ = done_tx.send(run_child(exe, &runs[i], grants, dir, i));
            }).expect("check worker");
        }
        drop(done_tx);
        let mut results: Vec<Option<Done>> = (0..runs.len()).map(|_| None).collect();
        let mut queue = 0..runs.len();
        let (mut running, mut next_out) = (0, 0);
        let mut peaks: Vec<u64> = Vec::new();
        let mut at_once = width;
        loop {
            while running < at_once {
                let Some(i) = queue.next() else { break };
                let _ = work_tx.send(i);
                running += 1;
            }
            if running == 0 {
                break;
            }
            let Ok(d) = done_rx.recv() else { break };
            running -= 1;
            worst = worst.max(d.code);
            if d.peak > 0 {
                peaks.push(d.peak);
                let now = fitting(width, &mut peaks, ram);
                if now != at_once {
                    eprintln!("relax check: {now} packages at once (was {at_once}): {width} times the checks' peak memory ({} MB, the p95 of {}) is {} half this machine's {} MB",
                        p95(&mut peaks) >> 20, peaks.len(), if now < at_once { "over" } else { "within" }, ram >> 20);
                    at_once = now;
                }
            }
            let index = d.index;
            results[index] = Some(d);
            while next_out < results.len() {
                let Some(d) = results[next_out].take() else { break };
                write_out(&runs[next_out].package, &d);
                next_out += 1;
            }
        }
        drop(work_tx);
    });
    worst
}

/// How many children at once: `width`, or fewer while that many of the children's p95 peak
/// would be over half of `ram`.
fn fitting(width: usize, peaks: &mut [u64], ram: u64) -> usize {
    let p = p95(peaks).max(1);
    if (width as u64).saturating_mul(p) <= ram / 2 { width } else { ((ram / 2 / p) as usize).clamp(1, width) }
}

fn p95(peaks: &mut [u64]) -> u64 {
    if peaks.is_empty() {
        return 0;
    }
    peaks.sort_unstable();
    peaks[(peaks.len() * 95).div_ceil(100) - 1]
}

/// A child's output written as the serial run's: its stdout, its stderr, the exit line.
fn write_out(package: &str, d: &Done) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&d.out);
    let _ = out.flush();
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(&d.err);
    if d.code != 0 {
        let _ = writeln!(err, "relax check: {package}: exit {}", d.code);
    }
    let _ = err.flush();
}

/// One package's check to its end: what it wrote, its exit (2: it did not start or exit on its
/// own, as the serial run counts it) and its peak memory.
fn run_child(exe: &Path, run: &Run, grants: &[String], dir: &Path, index: usize) -> Done {
    let name = |ext: &str| dir.join(format!("{}-{index}.{ext}", std::process::id()));
    let (out_path, err_path) = (name("out"), name("err"));
    let mut done = Done { index, out: Vec::new(), err: Vec::new(), code: 2, peak: 0 };
    let files = (|| Ok::<_, std::io::Error>((crate::os::proc::create_private(&out_path)?, crate::os::proc::create_private(&err_path)?)))();
    match files {
        Ok((out, err)) => {
            let child = Command::new(exe).arg("check").args(grants).args(&run.args)
                .env("RELAX_JOBS", CHILD_JOBS.to_string()).env("MAKEPAD_CPU_CORES", CHILD_JOBS.to_string())
                .env("RELAX_CHECK_GROUP", crate::store::pass_token())
                .stdin(Stdio::null()).stdout(out).stderr(err).spawn();
            if let Ok(mut child) = child {
                let (code, peak) = wait_peak(&mut child);
                done.code = code.unwrap_or(2);
                done.peak = peak;
            }
            done.out = std::fs::read(&out_path).unwrap_or_default();
            done.err = std::fs::read(&err_path).unwrap_or_default();
        }
        Err(e) => done.err = format!("relax check: {}: {e}\n", dir.display()).into_bytes(),
    }
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    done
}

/// A child's exit code (None: ended by a signal) and its peak resident memory in bytes (0: not
/// known), once it ended.
#[cfg(unix)]
fn wait_peak(child: &mut std::process::Child) -> (Option<i32>, u64) {
    use std::os::unix::process::ExitStatusExt;
    extern "C" {
        // struct rusage: two timevals (16 bytes each), then ru_maxrss and 13 more longs
        fn wait4(pid: i32, status: *mut i32, options: i32, rusage: *mut [i64; 18]) -> i32;
    }
    let mut status = 0i32;
    let mut usage = [0i64; 18];
    loop {
        if unsafe { wait4(child.id() as i32, &mut status, 0, &mut usage) } >= 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return (child.wait().ok().and_then(|s| s.code()), 0);
        }
    }
    // ru_maxrss: kilobytes on Linux, bytes on macOS
    let peak = usage[4].max(0) as u64;
    let peak = if cfg!(target_vendor = "apple") { peak } else { peak << 10 };
    (std::process::ExitStatus::from_raw(status).code(), peak)
}

#[cfg(windows)]
fn wait_peak(child: &mut std::process::Child) -> (Option<i32>, u64) {
    use std::os::windows::io::AsRawHandle;
    let code = child.wait().ok().and_then(|s| s.code());
    // PROCESS_MEMORY_COUNTERS_EX: cb and PageFaultCount, then PeakWorkingSetSize
    let mut c = [0usize; 10];
    c[0] = std::mem::size_of_val(&c);
    let ok = unsafe { relax_session::win32::K32GetProcessMemoryInfo(child.as_raw_handle() as relax_session::win32::HANDLE, c.as_mut_ptr().cast(), std::mem::size_of_val(&c) as u32) };
    (code, if ok != 0 { c[1] as u64 } else { 0 })
}

#[cfg(not(any(unix, windows)))]
fn wait_peak(child: &mut std::process::Child) -> (Option<i32>, u64) {
    (child.wait().ok().and_then(|s| s.code()), 0)
}

/// Each `-p` package with the arguments of its check: the command's own, the package standing
/// where the first `-p` stood (`check [--json] <package> [target flags]` reads the flags after
/// the package as its target flags). Without `-p`, the arguments as written (no package name).
fn package_args(args: &[String]) -> Result<Vec<Run>, i32> {
    let mut rest = Vec::new();
    let mut packages = Vec::new();
    let mut at = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" => {
                let Some(p) = args.get(i + 1).filter(|p| !p.starts_with('-')) else {
                    eprintln!("relax check: {} needs a package", args[i]);
                    return Err(2);
                };
                at.get_or_insert(rest.len());
                packages.push(p.clone());
                i += 2;
            }
            _ => {
                rest.push(args[i].clone());
                i += 1;
            }
        }
    }
    let Some(at) = at else { return Ok(vec![Run { package: String::new(), args: rest }]) };
    Ok(packages.into_iter().map(|p| {
        let mut args = rest.clone();
        args.insert(at, p.clone());
        Run { package: p, args }
    }).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_package_stands_before_the_target_flags() {
        let words = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        let runs = |s: &str| super::package_args(&words(s)).unwrap().into_iter().map(|r| r.args.join(" ")).collect::<Vec<_>>();
        assert_eq!(runs("-p a -p b --tests"), ["a --tests", "b --tests"], "several packages: each before the target flags");
        assert_eq!(runs("--json -p a --test t -p b"), ["--json a --test t", "--json b --test t"], "a leading flag stays in front");
        assert_eq!(runs("-p a --lib"), ["a --lib"], "one package keeps its place before the target flags");
        assert_eq!(runs("a --tests"), ["a --tests"], "without -p the arguments stay as written");
        assert_eq!(super::package_args(&words("-p --tests")).err(), Some(2), "-p without a package is a usage error");
    }

    #[test]
    fn the_width_follows_the_measured_peak() {
        const GB: u64 = 1 << 30;
        let mut peaks = vec![GB; 20];
        assert_eq!(super::fitting(8, &mut peaks, 64 * GB), 8, "8 x 1 GB is within half of 64 GB");
        let mut peaks = vec![6 * GB; 20];
        assert_eq!(super::fitting(8, &mut peaks, 64 * GB), 5, "8 x 6 GB is over 32 GB: 5 fit");
        let mut peaks: Vec<u64> = (0..20).map(|i| if i == 0 { 40 * GB } else { GB }).collect();
        assert_eq!(super::fitting(8, &mut peaks, 64 * GB), 8, "one outlier above the p95 does not lower it");
        let mut peaks = vec![64 * GB];
        assert_eq!(super::fitting(8, &mut peaks, 64 * GB), 1, "never below one");
    }
}

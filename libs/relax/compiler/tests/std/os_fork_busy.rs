// A program that forks (Command + pre_exec: std's fork path) while its other threads keep the
// JIT busy: lazy compiles of fresh functions, allocation, formatting, stderr, thread starts.
// Under `makepad run` on Linux the forked child once blocked on a runtime lock a parent thread
// held at the fork and never reached exec (makepad-app-aihub: GpuCache -> nvidia-smi with a
// PR_SET_PDEATHSIG pre_exec, 2026-10-24). Every pre_exec closure here is a fresh instantiation
// (not compiled before its fork) so that the child is where its code is first reached.
// Lines starting with "| " must equal real std's (os_diff.sh / core_diff.sh). The same file is the
// `makepad run relax-fork-busy` program (tools/run/fixture/src/fork_busy.rs, fork_busy.sh).
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);
static SINK: AtomicUsize = AtomicUsize::new(0);

/// A chain of distinct functions: each call reaches N new ones the JIT compiles lazily.
#[inline(never)]
fn chain<const N: usize>(x: usize) -> usize {
    let v: Vec<usize> = (0..(x % 7 + N % 5)).collect();
    let s = format!("{}{}", N, v.len());
    x.wrapping_mul(31).wrapping_add(s.len() + v.iter().sum::<usize>())
}

macro_rules! chains {
    ($x:expr; $($n:literal)*) => {{ let mut x = $x; $( x = chain::<$n>(x); )* x }};
}

/// Background work: new code (lazy compiles), allocation, formatting, stderr, thread starts.
fn busy(id: usize) {
    let mut round = 0usize;
    while !STOP.load(Ordering::Relaxed) {
        let x = match round % 4 {
            0 => chains!(id; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32),
            1 => chains!(id; 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64),
            2 => {
                let t = std::thread::spawn(move || chains!(round; 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79 80));
                t.join().unwrap()
            }
            _ => {
                let m: std::collections::BTreeMap<usize, String> = (0..64).map(|i| (i, format!("{i}:{id}"))).collect();
                m.values().map(|s| s.len()).sum()
            }
        };
        if round % 64 == 0 {
            eprint!("");
        }
        SINK.fetch_add(x, Ordering::Relaxed);
        round += 1;
    }
}

extern "C" {
    fn getppid() -> i32;
    fn getpid() -> i32;
    #[cfg(target_os = "linux")]
    fn prctl(op: i32, a: usize, b: usize, c: usize, d: usize) -> i32;
}

/// The code the child runs between fork and exec: fresh per instantiation; C calls as the AI
/// hub's (PR_SET_PDEATHSIG, then a parent check).
#[inline(never)]
fn in_child<const K: usize>() -> std::io::Result<()> {
    SINK.fetch_add(K, Ordering::Relaxed);
    #[cfg(target_os = "linux")]
    unsafe {
        if prctl(1, 9, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    if unsafe { getppid() == getpid() } {
        return Err(std::io::Error::from_raw_os_error(3));
    }
    Ok(())
}

/// The longest spawn() call so far, in microseconds (stderr: what a spawn waited for).
static SPAWN_MAX_US: AtomicUsize = AtomicUsize::new(0);

/// Starts `true` with a pre_exec closure and waits for it, at most 20 s.
fn fork_once<const K: usize>() -> bool {
    let mut c = Command::new("true");
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        c.pre_exec(|| in_child::<K>());
    }
    let t = Instant::now();
    let spawned = c.spawn();
    SPAWN_MAX_US.fetch_max(t.elapsed().as_micros() as usize, Ordering::Relaxed);
    let mut child = match spawned {
        Ok(ch) => ch,
        Err(_) => return false,
    };
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if t0.elapsed() < Duration::from_secs(20) => std::thread::sleep(Duration::from_millis(1)),
            _ => {
                eprintln!("os_fork_busy: child {} never reached exec (stuck between fork and exec)", child.id());
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

macro_rules! forks {
    ($ok:ident; $($k:literal)*) => { $( $ok += fork_once::<$k>() as usize; )* };
}

#[test]
fn a01_fork_while_jit_busy() {
    fork_while_jit_busy();
}

/// The test's body (also the main of `relax-fork-busy`).
pub fn fork_while_jit_busy() {
    STOP.store(false, Ordering::Relaxed);
    let workers: Vec<_> = (0..6).map(|i| std::thread::spawn(move || busy(i))).collect();
    let mut ok = 0usize;
    for _ in 0..4 {
        forks!(ok; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
                   33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50);
    }
    STOP.store(true, Ordering::Relaxed);
    for w in workers {
        w.join().unwrap();
    }
    println!("| forks that reached exec: {} of 200", ok);
}

const SIBLING_SPAWNS: usize = 100;

static HEAT_STOP: AtomicBool = AtomicBool::new(false);

/// Hot code for the tiers: each instantiation a slot of its own, whose calls and loop turn it hot
/// (its O2, then O3 builds on the compile workers while the spawns below fork).
#[inline(never)]
fn hot<const K: usize>(n: u64) -> u64 {
    let mut x = n ^ K as u64;
    for i in 0..2000u64 {
        x = x.rotate_left(5) ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    x
}

macro_rules! hots {
    ($n:expr; $($k:literal)*) => {{ let mut s = 0u64; $( s = s.wrapping_add(hot::<$k>($n)); )* s }};
}

fn heat() {
    let mut n = 0u64;
    while !HEAT_STOP.load(Ordering::Relaxed) {
        let x = hots!(n; 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32);
        SINK.fetch_add(x as usize, Ordering::Relaxed);
        n += 1;
    }
}

/// Spawns from four threads at once (each closure new: its code is built for that spawn, through
/// the compiler's owner, while another thread forks), with hot loops tiering up beside them. Under
/// `makepad run` a spawn's fork once waited for every compile job to end: for O3 builds, and for
/// good where a job waited on an owner request the pause held back (a devhub test gate hung 26 min
/// in a spawn, 2026-11-04).
#[test]
fn a03_concurrent_spawns_while_tiers_build() {
    concurrent_spawns_while_tiers_build();
}

/// The test's body (also run by `relax-fork-busy`).
pub fn concurrent_spawns_while_tiers_build() {
    HEAT_STOP.store(false, Ordering::Relaxed);
    let heaters: Vec<_> = (0..3).map(|_| std::thread::spawn(heat)).collect();
    let spawners = vec![
        std::thread::spawn(|| { let mut ok = 0usize; forks!(ok; 101 102 103 104 105 106 107 108 109 110 111 112); ok }),
        std::thread::spawn(|| { let mut ok = 0usize; forks!(ok; 201 202 203 204 205 206 207 208 209 210 211 212); ok }),
        std::thread::spawn(|| { let mut ok = 0usize; forks!(ok; 301 302 303 304 305 306 307 308 309 310 311 312); ok }),
        std::thread::spawn(|| { let mut ok = 0usize; forks!(ok; 401 402 403 404 405 406 407 408 409 410 411 412); ok }),
    ];
    let ok: usize = spawners.into_iter().map(|s| s.join().unwrap()).sum();
    HEAT_STOP.store(true, Ordering::Relaxed);
    for h in heaters {
        h.join().unwrap();
    }
    println!("| concurrent spawns that reached exec: {} of 48", ok);
    eprintln!("os_fork_busy: longest spawn {:.1} ms", SPAWN_MAX_US.load(Ordering::Relaxed) as f64 / 1e3);
}

/// Spawns from four threads whose children outlive them: no spawn waits for another spawn's child.
/// Between `pipe` and `fcntl(FD_CLOEXEC)` another thread's fork left this spawn's error pipe open in
/// its child, and the spawn read that pipe until that child ended (a devhub test's hub held its
/// lease table through it, and its tools waited for the run that never started, 2026-11-04). Real
/// std's `pipe2` has no such moment on Linux; macOS has no pipe2, in real std either.
#[test]
fn a04_spawns_never_wait_for_each_others_children() {
    spawns_never_wait_for_each_others_children();
}

/// The test's body (also run by `relax-fork-busy`).
pub fn spawns_never_wait_for_each_others_children() {
    const LIFE: Duration = Duration::from_secs(1);
    let waited = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let mut children = Vec::new();
                for _ in 0..SIBLING_SPAWNS {
                    let mut c = Command::new("sleep");
                    c.arg("1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    unsafe {
                        c.pre_exec(|| Ok(()));
                    }
                    let t = Instant::now();
                    children.push(c.spawn().unwrap());
                    if t.elapsed() >= LIFE {
                        waited.fetch_add(1, Ordering::Relaxed);
                    }
                }
                for mut c in children {
                    let _ = c.wait();
                }
            });
        }
    });
    println!("| spawns that waited for another spawn's child: {}", waited.load(Ordering::Relaxed));
}

trait Probe: Sync {
    fn go(&self) -> i32;
}

struct ParentCheck(i32);

impl Probe for ParentCheck {
    #[inline(never)]
    fn go(&self) -> i32 {
        let p = unsafe { getppid() };
        p + self.0
    }
}

/// A closure that reaches a fn only through a vtable made at run time (nothing the closure's
/// code names): under the JIT the child ends before exec at the unbuilt fn, the spawn builds it
/// and forks again -- the spawn succeeds as with real std, and never hangs.
#[test]
fn a02_pre_exec_through_a_vtable() {
    let o: &'static dyn Probe = Box::leak(Box::new(ParentCheck(std::env::args().count() as i32)));
    for round in 0..2 {
        let st = unsafe {
            Command::new("true")
                .pre_exec(move || if o.go() == i32::MIN { Err(std::io::Error::from_raw_os_error(1)) } else { Ok(()) })
                .status()
        };
        println!("| round {} status {:?}", round, st.map(|s| s.code()));
    }
}

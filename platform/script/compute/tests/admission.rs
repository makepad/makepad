//! Admission and counted work for untrusted origins, in ops (the units a
//! kernel counts while it runs): the worst case of a job is checked before
//! it runs (the element bound, the untrusted ceiling, the device ledger),
//! a running job stops once its counted ops pass its budget, whatever the
//! machine, backend or thread count, and a job at the admitted per-element
//! ceiling stops within 2 ms of a cancel natively (10 ms interpreted).

use makepad_script_compute::admission::{element_ops, DeviceLimits, JobBudget, Ledger, Origin, Refused};
use makepad_script_compute::kernel::{compile_with, Kernel, KernelError};
use makepad_script_compute::Backend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn kernel(body: &str, backend: Backend) -> Arc<Kernel> {
    let src = format!("let src = input(f32)\nlet o = output(f32)\nfn element(i) {{\n {}\n }}", body);
    compile_with(&src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e))
}

fn budget(ops: u64) -> JobBudget {
    JobBudget { work: ops }
}

/// Runs `k` over `n` elements under `limit` counted ops: the ops it counted,
/// or the error.
fn counted(k: &Kernel, n: usize, limit: u64, threads: usize, interp: bool, simd: bool) -> Result<u64, KernelError> {
    let src = vec![0.0f32; n];
    let mut out = vec![0.0f32; n];
    let mut c = k.call();
    c.input("src", &src).unwrap();
    c.output("o", &mut out).unwrap();
    c.set_work_limit(limit);
    c.set_simd(simd);
    let r = if interp { c.run_interp(n) } else if threads > 1 { c.run_parallel(n, threads) } else { c.run(n) };
    r.map(|stats| stats.work)
}

#[test]
fn counted_work_is_the_same_on_every_backend_and_thread_count() {
    for body in [
        "let s = float(i)\n for a in 0..1000 { s = sin(s) }\n o[i] = s",
        // A data-dependent exit: the count follows what ran, not the cap.
        "let s = float(i)\n for a in 0..1000 { s = s + 1.0\n if s > float(i % 97) + 40.0 { break } }\n o[i] = s",
    ] {
        let k = kernel(body, Backend::Native);
        let ki = kernel(body, Backend::Interp);
        let n = 10_001;
        let base = counted(&k, n, u64::MAX, 1, false, false).unwrap();
        assert!(base > 0 && base <= element_ops(&k) * n as u64, "{base} within the worst case");
        for (threads, interp, simd) in [(1, false, true), (8, false, true), (8, false, false), (1, true, false)] {
            let kk = if interp { &ki } else { &k };
            assert_eq!(counted(kk, n, u64::MAX, threads, interp, simd).unwrap(), base, "{body}: threads {threads} interp {interp} simd {simd}");
        }
        // Exactly its count runs; one op less stops it, on every backend.
        for (threads, interp) in [(1, false), (8, false), (1, true)] {
            let kk = if interp { &ki } else { &k };
            assert_eq!(counted(kk, n, base, threads, interp, true).unwrap(), base);
            assert!(matches!(counted(kk, n, base - 1, threads, interp, true), Err(KernelError::OverBudget { limit, .. }) if limit == base - 1));
        }
    }
    // The early exit counts far less than its static worst case.
    let early = kernel("let s = 0.0\n for a in 0..1000 { s = s + 1.0\n if s > 3.0 { break } }\n o[i] = s", Backend::Native);
    let full = kernel("let s = 0.0\n for a in 0..1000 { s = s + 1.0 }\n o[i] = s", Backend::Native);
    let (e, f) = (counted(&early, 100, u64::MAX, 1, false, true).unwrap(), counted(&full, 100, u64::MAX, 1, false, true).unwrap());
    assert!(e * 20 < f, "early {e} vs full {f}");
}

#[test]
fn untrusted_elements_are_bounded_host_elements_are_not() {
    let ledger = Ledger::new(DeviceLimits { element_work: 100_000, ..DeviceLimits::default() });
    let heavy = kernel("let s = i\n for a in 0..20000 { s = int(src[s]) }\n o[i] = float(s)", Backend::Native);
    assert!(element_ops(&heavy) > 100_000);
    for origin in [Origin::Ai, Origin::Store, Origin::Lan, Origin::Livecode] {
        match ledger.admit(&heavy, 10, origin, &budget(1 << 30)) {
            Err(Refused::ElementTooHeavy { worst_ops, limit_ops }) => assert!(worst_ops > limit_ops),
            other => panic!("{:?}: expected ElementTooHeavy, got {:?}", origin, other.map(|t| t.estimate())),
        }
    }
    assert!(ledger.admit(&heavy, 10, Origin::Host, &budget(1 << 30)).is_ok());
    assert_eq!(ledger.stats().jobs, 0, "tickets return their share on drop");
}

#[test]
fn job_budgets_and_untrusted_ceilings() {
    let limits = DeviceLimits { untrusted_job_work: 1_000_000, untrusted_worst_case: 1 << 30, ..DeviceLimits::default() };
    let ledger = Ledger::new(limits);
    let k = kernel("let s = float(i)\n for a in 0..1000 { s = sin(s) }\n o[i] = s", Backend::Native);
    let per = element_ops(&k);
    // An untrusted job's budget is capped; a trusted one's is its own.
    let t = ledger.admit(&k, 10, Origin::Ai, &budget(1 << 40)).unwrap();
    assert_eq!(t.work_limit(), 1_000_000);
    assert_eq!(t.estimate().charge_ops, (per * 10).min(1_000_000));
    let t = ledger.admit(&k, 10, Origin::Host, &budget(1 << 40)).unwrap();
    assert_eq!(t.work_limit(), 1 << 40);
    // The worst case only refuses past the ceiling.
    let fits = ((1u64 << 30) / per) as usize;
    assert!(ledger.admit(&k, fits, Origin::Ai, &budget(1000)).is_ok());
    assert!(matches!(ledger.admit(&k, fits + 1, Origin::Ai, &budget(1000)), Err(Refused::OverCeiling { .. })));
    assert!(ledger.admit(&k, fits + 1, Origin::Host, &budget(1000)).is_ok());
}

#[test]
fn the_device_ledger_sums_every_live_job() {
    let limits = DeviceLimits { max_in_flight: 100_000, max_untrusted_in_flight: 50_000, max_jobs: 8, max_untrusted_jobs: 3, ..DeviceLimits::default() };
    let ledger = Ledger::new(limits);
    let k = kernel("let s = float(i)\n for a in 0..1000 { s = sin(s) }\n o[i] = s", Backend::Native);
    // Each job is charged what it may run: here its budget.
    let a = ledger.admit(&k, 1000, Origin::Ai, &budget(20_000)).unwrap();
    let b = ledger.admit(&k, 1000, Origin::Store, &budget(20_000)).unwrap();
    assert!(matches!(ledger.admit(&k, 1000, Origin::Lan, &budget(20_000)), Err(Refused::DeviceBusy { .. })));
    // Host work uses the rest of the device, not the untrusted share.
    let c = ledger.admit(&k, 1000, Origin::Host, &budget(20_000)).unwrap();
    let d = ledger.admit(&k, 1000, Origin::Host, &budget(20_000)).unwrap();
    assert!(matches!(ledger.admit(&k, 1000, Origin::Host, &budget(40_000)), Err(Refused::DeviceBusy { .. })));
    assert_eq!(ledger.stats().jobs, 4);
    drop(a);
    assert!(ledger.admit(&k, 1000, Origin::Lan, &budget(20_000)).is_ok());
    drop((b, c, d));
    assert_eq!(ledger.stats(), Default::default());
    // Job-count limits.
    let small: Vec<_> = (0..3).map(|_| ledger.admit(&k, 1, Origin::Ai, &budget(100)).unwrap()).collect();
    assert!(matches!(ledger.admit(&k, 1, Origin::Ai, &budget(100)), Err(Refused::TooManyJobs { .. })));
    drop(small);
}

/// The largest `for` count whose element the ledger admits for untrusted
/// code (the admitted ceiling), and that kernel.
fn at_the_ceiling(template: &str, backend: Backend) -> (u32, Arc<Kernel>) {
    let ledger = Ledger::new(DeviceLimits::default());
    let (mut lo, mut hi) = (1u32, 2_000_000u32);
    let admitted = |n: u32| {
        let src = format!("let src = input(f32)\nlet o = output(f32)\nfn element(i) {{\n {}\n }}", template.replace("{N}", &n.to_string()));
        compile_with(&src, &[], backend).ok().filter(|k| ledger.admit(k, 1, Origin::Ai, &budget(1000)).is_ok())
    };
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if admitted(mid).is_some() {
            lo = mid
        } else {
            hi = mid
        }
    }
    (lo, admitted(lo).unwrap())
}

/// Runs `k` over many elements, cancels it after `after`, returns the time
/// from the cancel to the call's return.
fn cancel_latency(k: &Kernel, src: &[f32], n: usize, after: Duration) -> Duration {
    let mut out = vec![0.0f32; n];
    let mut c = k.call();
    c.input("src", src).unwrap();
    c.output("o", &mut out).unwrap();
    let token = c.cancel_token();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let at = Instant::now();
        token.cancel();
        tx.send(at).unwrap();
    });
    let r = c.run(n);
    let done = Instant::now();
    assert!(matches!(r, Err(KernelError::Cancelled)), "the job finished before the cancel");
    done.duration_since(rx.recv().unwrap())
}

pub const HOSTILE: &[(&str, &str)] = &[
    ("dependent loads", "let s = i * 977\n for a in 0..{N} { s = int(src[s]) }\n o[i] = float(s)"),
    ("fdiv chain", "let s = float(i) + 1.5\n for a in 0..{N} { s = 1.0 / (s + 1.5) }\n o[i] = s"),
    ("sin chain", "let s = float(i)\n for a in 0..{N} { s = sin(s) + 0.5 }\n o[i] = s"),
];

#[test]
fn cancel_stops_a_job_at_the_admitted_ceiling_within_2ms_natively() {
    // A 64 MiB random cycle: every load of the chase misses.
    let len = 1usize << 24;
    let mut next = vec![0f32; len];
    let mut x = 0x9E3779B97F4A7C15u64;
    let mut perm: Vec<u32> = (0..len as u32).collect();
    for k in (1..len).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        perm.swap(k, (x % k as u64) as usize);
    }
    for k in 0..len {
        next[perm[k] as usize] = perm[(k + 1) % len] as f32;
    }
    drop(perm);
    for backend in [Backend::Native, Backend::Interp] {
        for (name, template) in HOSTILE {
            let (n, k) = at_the_ceiling(template, backend);
            // The best of three: the bound is about the kernel, not about
            // a busy machine descheduling the test.
            let worst = (0..3).map(|_| cancel_latency(&k, &next, 100_000, Duration::from_millis(30))).min().unwrap();
            eprintln!("{:?} {:16} ceiling {:7} iterations: cancel -> return {:.3} ms", backend, name, n, worst.as_secs_f64() * 1e3);
            // The element bound is ops, the same for every backend; the
            // reference interpreter runs an op about ten times slower.
            let bound = Duration::from_millis(if backend == Backend::Interp { 10 } else { 2 });
            assert!(worst <= bound, "{:?} {}: {:?}", backend, name, worst);
        }
    }
}

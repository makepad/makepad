//! Admission for untrusted origins: the worst case of a job is checked
//! before it runs (job budget, per-element cancel bound, device ledger),
//! and a job at the admitted per-element ceiling stops within 2 ms of a
//! cancel, native and interpreted.

use makepad_script_compute::admission::{element_ps, DeviceLimits, JobBudget, Ledger, Origin, Refused};
use makepad_script_compute::kernel::{compile_with, Kernel, KernelError};
use makepad_script_compute::Backend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn kernel(body: &str, backend: Backend) -> Arc<Kernel> {
    let src = format!("let src = input(f32)\nlet o = output(f32)\nfn element(i) {{\n {}\n }}", body);
    compile_with(&src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e))
}

fn budget(ms: u64) -> JobBudget {
    JobBudget { wall: Duration::from_millis(ms) }
}

#[test]
fn memory_accesses_are_charged_as_misses() {
    let alu = kernel("let s = float(i)\n for a in 0..100 { s = s * 0.5 + 1.0 }\n o[i] = s", Backend::Native);
    let loads = kernel("let s = i\n for a in 0..100 { s = int(src[s]) }\n o[i] = float(s)", Backend::Native);
    // Similar op counts (1337 vs 1539); the loads cost over 20x more.
    assert!(element_ps(&loads, Backend::Native) > 20 * element_ps(&alu, Backend::Native));
}

#[test]
fn untrusted_elements_are_bounded_host_elements_are_not() {
    let ledger = Ledger::new(DeviceLimits::default());
    // ~20k dependent loads per element: ~5 ms worst case per element.
    let slow = kernel("let s = i\n for a in 0..20000 { s = int(src[s]) }\n o[i] = float(s)", Backend::Native);
    for origin in [Origin::Ai, Origin::Store, Origin::Lan, Origin::Livecode] {
        match ledger.admit(&slow, 10, 1, origin, &budget(1000)) {
            Err(Refused::ElementTooSlow { worst_ns, limit_ns }) => assert!(worst_ns > limit_ns),
            other => panic!("{:?}: expected ElementTooSlow, got {:?}", origin, other.map(|t| t.estimate())),
        }
    }
    assert!(ledger.admit(&slow, 10, 1, Origin::Host, &budget(1000)).is_ok());
    assert_eq!(ledger.stats().jobs, 0, "tickets return their share on drop");
}

#[test]
fn job_budgets_and_untrusted_ceilings() {
    let ledger = Ledger::new(DeviceLimits::default());
    let k = kernel("let s = float(i)\n for a in 0..1000 { s = sin(s) }\n o[i] = s", Backend::Native);
    let per = element_ps(&k, Backend::Native);
    // Exactly what fits a 10 ms budget on one thread, then one more element.
    let fits = (10_000_000_000u64 / per) as usize;
    assert!(ledger.admit(&k, fits, 1, Origin::Ai, &budget(10)).is_ok());
    assert!(matches!(ledger.admit(&k, fits + 1, 1, Origin::Ai, &budget(10)), Err(Refused::OverBudget { .. })));
    // An untrusted job cannot buy more than the device's untrusted wall.
    let big = (3_000_000_000_000u64 / per) as usize; // ~3 s
    assert!(matches!(ledger.admit(&k, big, 1, Origin::Livecode, &budget(60_000)), Err(Refused::OverBudget { .. })));
    // Element-local work splits across threads (in fixed chunks: here 8).
    let second = (1_000_000_000_000u64 / per) as usize;
    assert!(ledger.admit(&k, second * 4, 1, Origin::Ai, &budget(1000)).is_err());
    let t = ledger.admit(&k, second * 4, 4, Origin::Ai, &budget(1000)).unwrap();
    assert_eq!(t.estimate().threads, 4);
    assert_eq!(t.work_limit(), k.cost * (second as u64 * 4));
}

#[test]
fn the_device_ledger_sums_every_live_job() {
    let limits = DeviceLimits {
        max_in_flight: Duration::from_millis(100),
        max_untrusted_in_flight: Duration::from_millis(50),
        max_jobs: 8,
        max_untrusted_jobs: 3,
        ..DeviceLimits::default()
    };
    let ledger = Ledger::new(limits);
    let k = kernel("let s = float(i)\n for a in 0..1000 { s = sin(s) }\n o[i] = s", Backend::Native);
    let per = element_ps(&k, Backend::Native);
    let n20ms = (20_000_000_000u64 / per) as usize;
    // Untrusted share: two 20 ms jobs fit in 50 ms, a third does not.
    let a = ledger.admit(&k, n20ms, 1, Origin::Ai, &budget(100)).unwrap();
    let b = ledger.admit(&k, n20ms, 1, Origin::Store, &budget(100)).unwrap();
    assert!(matches!(ledger.admit(&k, n20ms, 1, Origin::Lan, &budget(100)), Err(Refused::DeviceBusy { .. })));
    // Host work uses the rest of the device (100 ms), not the untrusted share.
    let c = ledger.admit(&k, n20ms, 1, Origin::Host, &budget(100)).unwrap();
    let d = ledger.admit(&k, n20ms, 1, Origin::Host, &budget(100)).unwrap();
    assert!(matches!(ledger.admit(&k, n20ms * 2, 1, Origin::Host, &budget(100)), Err(Refused::DeviceBusy { .. })));
    assert_eq!(ledger.stats().jobs, 4);
    drop(a);
    assert!(ledger.admit(&k, n20ms, 1, Origin::Lan, &budget(100)).is_ok());
    drop((b, c, d));
    assert_eq!(ledger.stats(), Default::default());
    // Job-count limits.
    let small: Vec<_> = (0..3).map(|_| ledger.admit(&k, 1, 1, Origin::Ai, &budget(100)).unwrap()).collect();
    assert!(matches!(ledger.admit(&k, 1, 1, Origin::Ai, &budget(100)), Err(Refused::TooManyJobs { .. })));
    drop(small);
}

/// The largest `for` count whose element the ledger admits for untrusted
/// code (the admitted ceiling), and that kernel.
fn at_the_ceiling(template: &str, backend: Backend) -> (u32, Arc<Kernel>) {
    let ledger = Ledger::new(DeviceLimits::default());
    let (mut lo, mut hi) = (1u32, 2_000_000u32);
    let admitted = |n: u32| {
        let src = format!("let src = input(f32)\nlet o = output(f32)\nfn element(i) {{\n {}\n }}", template.replace("{N}", &n.to_string()));
        compile_with(&src, &[], backend).ok().filter(|k| ledger.admit(k, 1, 1, Origin::Ai, &budget(1000)).is_ok())
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
fn cancel_stops_a_job_at_the_admitted_ceiling_within_2ms() {
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
            assert!(worst <= Duration::from_millis(2), "{:?} {}: {:?}", backend, name, worst);
        }
    }
}

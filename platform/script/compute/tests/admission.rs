//! Admission and counted work for untrusted origins, in ops (the units a
//! kernel counts while it runs): the worst case of a job is checked before
//! it runs (the element bound, the untrusted ceiling, the device ledger),
//! a running job stops once its counted ops pass its budget (within one
//! slice), whatever the machine, backend or thread count, and a cancel
//! lands at the next element.

use makepad_script_compute::admission::{element_ops, DeviceLimits, JobBudget, Ledger, Origin, Refused};
use makepad_script_compute::kernel::{compile_with, Kernel, KernelError};
use makepad_script_compute::Backend;
use std::sync::Arc;


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

/// The element a test's cancel lands in, and the call to cancel.
static CANCEL_AT: std::sync::Mutex<Option<(u32, makepad_script_compute::kernel::CancelToken)>> = std::sync::Mutex::new(None);

/// `test.cancel_at(i)`: cancels the running call when its element `i` is
/// the chosen one; returns `i + 1`.
fn cancel_at(args: &[u32], _s: &[makepad_script_compute::host::HostSlice], rets: &mut [u32]) -> Result<(), makepad_script_compute::host::HostError> {
    if let Some((at, token)) = CANCEL_AT.lock().unwrap().as_ref() {
        if args[0] == *at {
            token.cancel();
        }
    }
    rets[0] = args[0] + 1;
    Ok(())
}

/// A cancel lands at the next element: what runs after it is at most the
/// element it came in, whose worst case admission bounds in ops (see
/// `untrusted_elements_are_bounded_host_elements_are_not`). The same on
/// every machine, at any load: the element that cancels is the last one
/// written, native and interpreted, whatever the element's own work.
#[test]
fn a_cancel_stops_the_call_at_the_next_element() {
    use makepad_script_compute::host::{self, HostFn, Tier};
    use makepad_script_compute::ir::Ty;
    let f = host::find("test.cancel_at").unwrap_or_else(|| {
        host::register(HostFn { name: "test.cancel_at", params: &[Ty::I32], slices: &[], rets: &[Ty::I32], cost: |_| 1, misses: 0, tier: Tier::D, call: cancel_at, doc: "" }).unwrap()
    });
    let _ = f;
    let src = "let o = output(i32)\nfn element(i) { let s = float(i)\n for a in 0..2000 { s = sin(s) + 0.5 }\n o[i] = test.cancel_at(i) + int(s) * 0 }";
    for backend in [Backend::Native, Backend::Interp] {
        let k = compile_with(src, &[], backend).unwrap_or_else(|e| panic!("{:?}", e));
        for at in [0u32, 7, 4095, 4096, 9000] {
            let n = 20_000;
            let mut out = vec![0u32; n];
            let mut c = k.call();
            c.output_u32("o", &mut out).unwrap();
            *CANCEL_AT.lock().unwrap() = Some((at, c.cancel_token()));
            let r = if backend == Backend::Interp { c.run_interp(n) } else { c.run(n) };
            *CANCEL_AT.lock().unwrap() = None;
            drop(c);
            assert!(matches!(r, Err(KernelError::Cancelled)), "{backend:?} at {at}: {r:?}");
            let last = out.iter().rposition(|w| *w != 0).map(|k| k as u32);
            assert_eq!(last, Some(at), "{backend:?}: the cancel came in element {at}");
        }
    }
}

/// A call stopped by its count stops close to its limit (in ops, the
/// same on every machine): at most four elements a worker past it.
#[test]
fn a_count_stops_the_call_within_four_elements_a_worker_of_its_limit() {
    let k = kernel("let s = float(i)\n for a in 0..2000 { s = sin(s) + 0.5 }\n o[i] = s", Backend::Native);
    let per = element_ops(&k);
    let ran = counted(&k, 1000, u64::MAX, 1, false, true).unwrap();
    assert!(ran <= per * 1000, "an element counts {} of its worst case {per}", ran / 1000);
    for threads in [1usize, 8] {
        for limit in [per * 10, per * 5000 + 17, per * 40_000] {
            match counted(&k, 100_000, limit, threads, false, true) {
                Err(KernelError::OverBudget { work, limit: l }) => {
                    assert_eq!(l, limit);
                    let most = 4 * threads as u64;
                    assert!(work > limit && work - limit <= most * per, "{threads} threads: {work} counted past {limit}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}

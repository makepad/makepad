//! The compiled-kernel store: a kernel loaded from stored code computes what a fresh compile
//! computes, also when the host trampoline it calls sits at another address (the next run).
//! The store holds the shared back end's code (both targets).
#![cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]

use makepad_script_compute::host::{self, ErrorSink, HostCallFrame, HostCallbacks, HostFn, Tier};
use makepad_script_compute::ir::Ty;
use makepad_script_compute::kcache;
use makepad_script_compute::kernel::{compile_with, KernelRef};
use makepad_script_compute::Backend;
use makepad_script_compute_sched::sched::{InlineExecutor, Job};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static STORE: Mutex<Option<HashMap<u128, Vec<u8>>>> = Mutex::new(None);
static PUTS: AtomicUsize = AtomicUsize::new(0);
static HITS: AtomicUsize = AtomicUsize::new(0);
static KEYS: Mutex<Vec<u128>> = Mutex::new(Vec::new());
static ALT_CALLS: AtomicUsize = AtomicUsize::new(0);

fn get(key: u128) -> Option<Vec<u8>> {
    let r = STORE.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned());
    if r.is_some() {
        HITS.fetch_add(1, Ordering::SeqCst);
    }
    r
}
fn put(key: u128, payload: Vec<u8>) {
    PUTS.fetch_add(1, Ordering::SeqCst);
    KEYS.lock().unwrap().push(key);
    STORE.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, payload);
}

unsafe extern "C-unwind" fn sum3(_context: *mut (), frame: &mut HostCallFrame, _errors: ErrorSink<'_>) -> bool {
    let (args, _s, rets) = frame.parts();
    rets[0] = (f32::from_bits(args[0]) + f32::from_bits(args[1]) + f32::from_bits(args[2])).to_bits();
    true
}
unsafe extern "C-unwind" fn scalar_cost(_: *mut (), _: *const u32, _: usize) -> u64 {
    1
}

/// Another address for the same trampoline: what a relocated loader image would present.
#[cfg(target_arch = "x86_64")]
unsafe extern "sysv64" fn alt_trampoline(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
    ALT_CALLS.fetch_add(1, Ordering::SeqCst);
    host::trampoline(rec, table, ctx, writable)
}
#[cfg(not(target_arch = "x86_64"))]
unsafe extern "C" fn alt_trampoline(rec: *mut u32, table: *const u64, ctx: *mut u32, writable: u64) {
    ALT_CALLS.fetch_add(1, Ordering::SeqCst);
    host::trampoline(rec, table, ctx, writable)
}

fn run(k: &KernelRef, n: usize, words: usize) -> Vec<u32> {
    let mut j = Job::new(k.clone(), n);
    j.output_u32("o", vec![0; n * words]).unwrap();
    j.run(&InlineExecutor, 1).unwrap();
    j.out_u32("o").unwrap().to_vec()
}

#[test]
fn stored_kernels_compute_what_a_fresh_compile_computes_relocated_or_not() {
    assert!(kcache::install(kcache::Hook { get, put }));
    host::register(HostFn {
        name: "test.sum3",
        params: &[Ty::F32, Ty::F32, Ty::F32],
        slices: &[],
        rets: &[Ty::F32],
        callbacks: unsafe { HostCallbacks::new(std::ptr::null_mut(), scalar_cost, sum3) },
        misses: 0,
        tier: Tier::X,
        doc: "",
    })
    .into_result()
    .unwrap();
    // a scalar kernel with a host call (the trampoline address is in its code), and a four-wide one
    let scalar = "let o = output(vec4)\nfn element(i) { let a = float(i) * 3.0\n let b = sin(a)\n let c = test.sum3(a, b, 1.0)\n o[i] = vec4(a, b, c, a + b + c) }";
    let wide = "let o = output(vec4)\nfn element(i) { let a = float(i) * 0.25\n o[i] = vec4(a, sin(a), cos(a) * a, a * a + 1.0) }";
    for src in [scalar, wide] {
        let reference = run(&compile_with(src, &[], Backend::Interp).unwrap(), 37, 4);
        let (puts, hits) = (PUTS.load(Ordering::SeqCst), HITS.load(Ordering::SeqCst));
        let fresh = compile_with(src, &[], Backend::Native).unwrap();
        assert!(PUTS.load(Ordering::SeqCst) > puts, "a fresh compile offers its code");
        assert_eq!(run(&fresh, 37, 4), reference);
        // from the store: same results, no compile
        let puts = PUTS.load(Ordering::SeqCst);
        let stored = compile_with(src, &[], Backend::Native).unwrap();
        assert_eq!(PUTS.load(Ordering::SeqCst), puts, "a stored kernel is not compiled again");
        assert!(HITS.load(Ordering::SeqCst) > hits);
        assert_eq!(stored.simd(), fresh.simd());
        assert_eq!(run(&stored, 37, 4), reference);
        // relocated: the host call goes to another address
        if src == scalar {
            kcache::override_host_addr(alt_trampoline as *const () as usize as u64);
            let moved = compile_with(src, &[], Backend::Native).unwrap();
            let calls = ALT_CALLS.load(Ordering::SeqCst);
            assert_eq!(run(&moved, 37, 4), reference);
            assert!(ALT_CALLS.load(Ordering::SeqCst) > calls, "the relocated code calls the new address");
            kcache::override_host_addr(0);
        }
    }
    // torn or foreign payloads are absent: a cut, a flipped byte, an empty one all recompile
    let key = *KEYS.lock().unwrap().last().unwrap();
    let good = STORE.lock().unwrap().as_ref().unwrap()[&key].clone();
    let reference = run(&compile_with(wide, &[], Backend::Interp).unwrap(), 37, 4);
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x40;
    for bad in [Vec::new(), good[..3].to_vec(), good[..good.len() / 2].to_vec(), good[..good.len() - 1].to_vec(), flipped] {
        STORE.lock().unwrap().as_mut().unwrap().insert(key, bad);
        let puts = PUTS.load(Ordering::SeqCst);
        let k = compile_with(wide, &[], Backend::Native).unwrap();
        assert!(PUTS.load(Ordering::SeqCst) > puts, "a damaged payload reads as absent");
        assert_eq!(run(&k, 37, 4), reference);
    }
}

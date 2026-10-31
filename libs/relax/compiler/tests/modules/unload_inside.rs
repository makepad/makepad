// Module unload (MEMRECLAIM.REVIEW F2, user decision 2026-10-23): an unloaded module's memory is
// never used again. Its pages go back, its address range stays reserved and inaccessible, and its
// arena stays as a tombstone that refuses entry. A stale access into the range is never contained:
// the process ends with a report naming the module.
// - default: calls after the unload are refused; windows of later modules never land on a dead one
//   (the dead windows stay counted, the live arena count does not grow with them).
// - SAFE2_UNLOAD_CRASH=parked: a thread parked inside the module (in a host call it made) when
//   the module is unloaded resumes into its window -> the process ends (JIT code faulting).
// - SAFE2_UNLOAD_CRASH=trusted_call (trusted modules): a call of an unloaded module's fn through a
//   kept fn pointer runs into its invalidated code -> parked / the process ends (INVALID).
// - SAFE2_UNLOAD_CRASH=host: host code keeps a pointer the module handed it and reads it after the
//   unload -> the process ends (host code faulting).
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub static PARKED: AtomicBool = AtomicBool::new(false);
pub static RELEASE: AtomicBool = AtomicBool::new(false);
pub static KEPT: AtomicU64 = AtomicU64::new(0);

/// Part of the host's API the parker module calls: waits until the test lets it go.
pub fn host_park(x: u64) -> u64 {
    PARKED.store(true, Ordering::SeqCst);
    while !RELEASE.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    x
}

/// Host API that (wrongly) keeps the address of data the module passed in.
pub fn host_keep(v: &[u64]) -> u64 {
    KEPT.store(v.as_ptr() as u64, Ordering::SeqCst);
    v.len() as u64
}

const PARKER: &str = r#"
pub fn parked(x: u64) -> u64 {
    // heap and shadow-stack state that lives across the host call
    let v: Vec<u64> = (0..4096u64).map(|i| i ^ x).collect();
    let mut local = [0u64; 64];
    for i in 0..64 { local[i] = v[i * 7] + 1; }
    let back = test_crate::host_park(x);
    let mut s = back;
    for i in 0..64 { s = s.wrapping_add(local[i]); }
    for e in v.iter() { s = s.wrapping_add(*e); }
    s
}
pub fn hand_over() -> u64 {
    let v: Vec<u64> = (0..512u64).collect();
    let n = test_crate::host_keep(&v);
    std::mem::forget(v);
    n
}
pub fn f(x: u64) -> u64 { let v = vec![x; 512]; v.iter().sum() }
"#;

#[test]
fn unload_inside() {
    use std::relax::Module;
    let mode = std::env::var("SAFE2_UNLOAD_CRASH").unwrap_or_default();
    let parker = Module::load_source("parker", "parker.rs", PARKER, &["test_crate"]).unwrap();
    if mode == "parked" {
        let parked: fn(u64) -> u64 = parker.get("parked").unwrap();
        let t = std::thread::spawn(move || std::relax::guard(0, || parked(41)));
        while !PARKED.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        parker.unload();
        RELEASE.store(true, Ordering::SeqCst);
        // not reached: the parked call's next access into the dead window ends the process,
        // even under a guard
        let r = t.join();
        println!("out parked call after unload was contained: {:?}", r.map(|g| g.is_ok()));
        return;
    }
    if mode == "trusted_call" {
        // MAKEPAD_RUN_MODULES_TRUSTED=1: the export is the module fn's own slot; after the unload its
        // code is invalidated (invalid.rs) and a call through the kept fn pointer is a stale access
        let f: fn(u64) -> u64 = parker.get("f").unwrap();
        assert_eq!(f(2), 1024);
        parker.unload();
        // not reached past this call
        let v = f(3);
        println!("out called an unloaded module's code: {}", v);
        return;
    }
    if mode == "host" {
        let hand_over: fn() -> u64 = parker.get("hand_over").unwrap();
        assert_eq!(hand_over(), 512);
        parker.unload();
        let p = KEPT.load(Ordering::SeqCst) as *const u64;
        // not reached past this read
        let v = unsafe { std::ptr::read_volatile(p.add(5)) };
        println!("out host read a dead window: {}", v);
        return;
    }
    let f: fn(u64) -> u64 = parker.get("f").unwrap();
    assert_eq!(f(2), 1024);
    let before = std::relax::sandbox_stats();
    parker.unload();
    // the tombstone refuses entry (the wrapper panics "unloaded")
    let refused = std::relax::guard(0, || f(2)).is_err();
    println!("out calls after unload are refused: {}", refused);
    // load / unload cycles: every dead window stays (counted), live arenas do not pile up
    for k in 0..40 {
        let m = Module::load_source(&format!("cycle{}", k), "cycle.rs", "pub fn f(x: u64) -> u64 { let v = vec![x; 512]; v.iter().sum() }", &[]).unwrap();
        let g: fn(u64) -> u64 = m.get("f").unwrap();
        assert_eq!(g(k), k * 512);
        m.unload();
    }
    let after = std::relax::sandbox_stats();
    println!("out dead windows stay reserved: {}", after[4] == before[4] + 41 && after[3] == before[3] - 1);
}

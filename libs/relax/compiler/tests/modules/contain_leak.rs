// A fault contained inside fault-safe host code (a module's copy into an unmapped page of its
// window) leaves the thread as it found it: a later fault in other host code on the same thread
// still ends the process, as JITSAFE fix 7 requires (a fault there may leave a lock held or state
// half made, which no jump can cut away). The contained fault used to leave the thread's
// fault-safe depth raised, and every later host-code fault on the thread was contained too.
// run.sh expects this run to end by the second fault, after the first line.
// Run: makepad test --allow-unchecked-modules tests/modules/contain_leak.rs
#[test]
fn contain_leak() {
    use std::relax::Module;
    let m = Module::load_source(
        "leak",
        "leak.rs",
        r#"
pub fn big_copy(a: u64) -> u64 {
    let src = vec![7u8; 1 << 20];
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), a as *mut u8, 1 << 20); }
    2
}
"#,
        &[],
    )
    .unwrap();
    let b: fn(u64) -> u64 = m.get("big_copy").unwrap();
    println!("out module fault contained: {}", std::relax::guard(1000, || b(0)).is_err());
    // host code, no module call: the runtime reads a bad text pointer, a fault in host code
    let r = std::relax::guard(1000, || unsafe { core::intrinsics_rt::panic_impl(16 as *const u8, 16, "x".as_ptr(), 1, 1, 1) });
    println!("out host fault contained: {}", r.is_err());
}

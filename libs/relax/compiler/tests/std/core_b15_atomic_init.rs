// boot b15 (std-os, std core::sync::atomic): the deprecated consts ATOMIC_USIZE_INIT /
// ATOMIC_ISIZE_INIT / ATOMIC_BOOL_INIT are missing ("cannot find value"); objc-sys class!/sel!
// statics use `::std::sync::atomic::ATOMIC_USIZE_INIT`. rustc: 0 (deprecation warning only).
#[allow(deprecated)]
static C: std::sync::atomic::AtomicUsize = std::sync::atomic::ATOMIC_USIZE_INIT;
fn f() -> usize {
    C.load(std::sync::atomic::Ordering::Relaxed)
}
#[test]
fn t() {
    assert!(f() == 0);
}

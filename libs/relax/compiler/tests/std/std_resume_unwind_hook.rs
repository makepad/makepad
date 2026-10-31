// panic::resume_unwind does not run the panic hook; panic_any does (std docs; tests/ui
// panics/panic-recover-propagate.rs)
use std::sync::atomic::{AtomicUsize, Ordering};
static A: AtomicUsize = AtomicUsize::new(0);
#[test]
fn resume_unwind_skips_hook() {
    std::panic::set_hook(Box::new(|_| { A.fetch_add(1, Ordering::SeqCst); }));
    let r = std::panic::catch_unwind(|| panic!("hi there"));
    assert_eq!(A.load(Ordering::SeqCst), 1);
    let p = std::panic::AssertUnwindSafe(r.unwrap_err());
    let r2 = std::panic::catch_unwind(move || { let p = p; std::panic::resume_unwind(p.0) });
    assert_eq!(A.load(Ordering::SeqCst), 1);
    assert_eq!(*r2.unwrap_err().downcast::<&'static str>().unwrap(), "hi there");
    let _ = std::panic::catch_unwind(|| std::panic::panic_any(7u32));
    assert_eq!(A.load(Ordering::SeqCst), 2);
    let _ = std::panic::take_hook();
}

// Run in a fresh process: populate the host hook before the first sandbox placement.

#[test]
fn panic_after_host_catch_and_hook() {
    std::panic::catch_unwind(|| ()).unwrap();
    // A populated, heap-owning std hook must not enter the sandbox image either.
    let label = String::from("host hook");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_calls = calls.clone();
    std::panic::set_hook(Box::new(move |_| {
        assert_eq!(label.len(), 9);
        hook_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let module = std::relax::Module::load_source("panic_after_catch", "panic_after_catch.rs", r#"
        static INITIAL: u64 = 37;
        static TEXT: &str = "module-local";
        pub fn value() -> u64 { INITIAL + TEXT.len() as u64 }
        pub fn fail() { panic!("contained module panic"); }
    "#, &[]).unwrap();
    let value: fn() -> u64 = module.get("value").unwrap();
    let fail: fn() = module.get("fail").unwrap();
    assert_eq!(std::relax::guard(1000, || value()).unwrap(), 49);
    assert!(std::relax::guard(1000, || fail()).is_err());
    assert_eq!(std::relax::guard(1000, || value()).unwrap(), 49);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    // The host retains its independent hook/TLS and can still catch its own panic.
    assert!(std::panic::catch_unwind(|| panic!("host still running")).is_err());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let _ = std::panic::take_hook();
    module.unload();
}

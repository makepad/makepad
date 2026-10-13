// Standalone Relax regression reproducer, not a Cargo integration test.
// Run with `makepad test <this file>` on node165; see VALIDATION.md.
// It must load successfully even after a host has used catch_unwind.
#[test]
fn load_after_host_catch() {
    std::panic::catch_unwind(|| ()).unwrap();
    let module = std::relax::Module::load_source("after_catch", "after_catch.rs", r#"
        pub fn copy(values: &mut [f64]) {
            let mut v = vec![0.0; values.len()];
            v.copy_from_slice(values);
            values.copy_from_slice(&v);
        }
    "#, &[]).unwrap();
    let copy: fn(&mut [f64]) = module.get("copy").unwrap();
    let mut values = [1.0, 2.0];
    std::relax::guard(20, || copy(&mut values)).unwrap();
    assert_eq!(values, [1.0, 2.0]);
    module.unload();
}

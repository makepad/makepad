// Fuel (hotreload.md §12.3): with MAKEPAD_RUN_MODULE_FUEL=1000000, a module call that runs more loop
// iterations than that ends with an error (no deadline needed); a shorter one completes, and
// the budget is per call (refilled at each entry).
#[test]
fn fuel() {
    use std::relax::Module;
    let m = Module::load_source("spin", "spin.rs", "pub fn spin(n: u64) -> u64 {\n    let mut x = 1u64;\n    let mut i = 0;\n    while i < n {\n        x = x.wrapping_mul(6364136223846793005).wrapping_add(i);\n        i += 1;\n    }\n    x\n}\n", &[]).unwrap();
    let f: fn(u64) -> u64 = m.get("spin").unwrap();
    // Fuel must still trap after O3 optimises a hot twin.
    for _ in 0..10000 { std::hint::black_box(f(10)); }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let short = std::relax::guard(0, || f(10_000));
    let long = std::relax::guard(0, || f(100_000_000));
    let again = std::relax::guard(0, || f(10_000));
    println!("out short ok: {}", short.is_ok());
    println!("out long stopped: {}", long.is_err());
    println!("out refilled: {}", again.is_ok() && again.ok() == short.ok());
}

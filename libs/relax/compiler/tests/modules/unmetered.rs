// Metering follows the grant (run.sh runs this file twice). Metered, the default: a module call
// pays fuel for its work, and a Budget smaller than that work ends the call. Granted unmetered
// modules (--allow-unmetered-modules): the module's code carries no fuel accounting at all, so
// the same call is charged nothing and completes.
use std::relax::{Budget, BudgetError, Module};

#[test]
fn metering_follows_the_grant() {
    let m = Module::load_source("unmetered_spin", "unmetered_spin.rs", "pub fn spin(n: u64) -> u64 {\n    let mut x = 1u64;\n    let mut i = 0;\n    while i < n {\n        x = x.wrapping_mul(6364136223846793005).wrapping_add(i);\n        i += 1;\n    }\n    x\n}\n", &[]).unwrap();
    let spin: fn(u64) -> u64 = m.get("spin").unwrap();
    let mut budget = Budget::new(10_000, 1 << 20);
    let long = m.call_with(&mut budget, || spin(1_000_000));
    println!("out long call completes: {}", long.is_ok());
    println!("out stopped for fuel: {}", matches!(long, Err(BudgetError::Fuel { .. })));
    println!("out fuel charged: {}", budget.consumed_fuel() > 0);
}

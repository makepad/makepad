// RELAX_SANDBOX_FUEL=0 leaves the meter out of a twin's code: a call spends no fuel and no
// budget ends it (a host that bounds its modules by deadline alone). By default the same call
// ends on its budget.
use std::relax::{Budget, BudgetError, Module};

const SOURCE: &str = r#"
pub fn sum(n: u64) -> u64 {
    let mut s = 0u64;
    let mut i = 0;
    while i < n { s = s.wrapping_mul(31).wrapping_add(i); i += 1; }
    s
}
"#;

#[test]
fn fuel_off() {
    let m = Module::load_source("fueloff", "fueloff.rs", SOURCE, &[]).unwrap();
    let sum: fn(u64) -> u64 = m.get("sum").unwrap();
    let mut budget = Budget::new(100, 16 * 1024 * 1024);
    let r = m.call_with(&mut budget, || sum(1000));
    let how = match r {
        Ok(v) => format!("{}", v == (0..1000u64).fold(0u64, |s, i| s.wrapping_mul(31).wrapping_add(i))),
        Err(BudgetError::Fuel { .. }) => "out of fuel".to_string(),
        Err(e) => format!("{:?}", e),
    };
    println!("out a budget of 100 units: result {} fuel spent {}", how, budget.consumed_fuel());
    m.unload();
}

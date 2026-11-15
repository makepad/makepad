// A twin's O3 build takes in the twins it calls and keeps the fuel count in a register between
// calls. Neither may show: the fuel a path costs, the instruction it runs out at and what the
// module did up to there are the same as before the tier-up. `run` calls small functions in a
// loop and the host every eighth turn; the host counts those calls, so a run that ends on its
// budget tells where it ended.
use std::relax::{Budget, BudgetError, Host, Module};

pub struct Tally { pub ticks: u64 }
pub fn tick(h: Host<Tally>) -> bool {
    h.with(|t| t.ticks += 1).is_ok()
}

const CAP: u64 = 16 * 1024 * 1024;
const SOURCE: &str = r#"
use std::relax::Host;
use test_crate::{tick, Tally};
fn mix(x: u64, i: u64) -> u64 {
    x.wrapping_mul(6364136223846793005).wrapping_add(i).rotate_left(7)
}
fn step(v: &mut [u64; 16], i: u64) -> u64 {
    let k = (i as usize) % v.len();
    v[k] = mix(v[k], i);
    v[k]
}
pub fn run(h: Host<Tally>, n: u64) -> u64 {
    let mut v = [1u64; 16];
    let mut acc = 0u64;
    let mut i = 0;
    while i < n {
        acc ^= step(&mut v, i);
        if i % 8 == 0 { tick(h); }
        i += 1;
    }
    acc
}
"#;

/// (fuel consumed, ticks the host saw, the result or how the call ended)
fn measure(m: &Module, run: fn(Host<Tally>, u64) -> u64, fuel: u64, n: u64) -> (u64, u64, String) {
    let mut tally = Tally { ticks: 0 };
    let mut budget = Budget::new(fuel, CAP);
    let r = m.with_host(&mut tally, |h| m.call_with(&mut budget, || run(h, n))).unwrap();
    let how = match r {
        Ok(v) => format!("{:016x}", v),
        Err(BudgetError::Fuel { .. }) => "out of fuel".to_string(),
        Err(e) => format!("{:?}", e),
    };
    (budget.consumed_fuel(), tally.ticks, how)
}

#[test]
fn fuel_tiers() {
    let m = Module::load_source("tiers", "tiers.rs", SOURCE, &["test_crate"]).unwrap();
    let run: fn(Host<Tally>, u64) -> u64 = m.get("run").unwrap();
    let whole = measure(&m, run, 100_000_000, 2000);
    let cut = measure(&m, run, whole.0 / 3, 2000);
    let exact = measure(&m, run, whole.0, 2000);
    let short = measure(&m, run, whole.0 - 1, 2000);
    // warm every function of the loop, then leave the background compiler time to install
    let mut tally = Tally { ticks: 0 };
    m.with_host(&mut tally, |h| for _ in 0..200 { std::hint::black_box(run(h, 400)); }).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let again = (measure(&m, run, 100_000_000, 2000), measure(&m, run, whole.0 / 3, 2000), measure(&m, run, whole.0, 2000), measure(&m, run, whole.0 - 1, 2000));
    println!("out whole run: fuel {} ticks {} result {}", whole.0, whole.1, whole.2);
    println!("out a third of its fuel: fuel {} ticks {} result {}", cut.0, cut.1, cut.2);
    println!("out exactly its fuel: {}", exact.2 == whole.2);
    println!("out one unit less: ticks {} result {}", short.1, short.2);
    println!("out the same after the tier-up: {}", again == (whole, cut, exact, short));
    m.unload();
}

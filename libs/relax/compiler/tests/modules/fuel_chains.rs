// A twin's O3 build checks a run of meters once (sandbox.rs merge_meters). That may not show:
// with every budget from one unit up to what a call costs, the call consumes the same fuel, ends
// the same way and has told the host the same things as with one check per block. `run` is
// straight-line arithmetic (rotations: register-only stretches), a local array (memory between
// the meters), a division and a branch; it hands the host a value at several points of each
// turn, so the host's journal tells where a call on a budget stopped and what it had computed.
// `walk` leaves a chain another way: its last turns index past an array, a panic that reports
// that turn's values.
use std::relax::{Budget, BudgetError, Host, Module};

pub struct Journal { pub notes: u64, pub fold: u64 }
pub fn note(h: Host<Journal>, x: u64) -> bool {
    h.with(|j| { j.notes += 1; j.fold = (j.fold.rotate_left(5) ^ x).wrapping_mul(0x100000001b3); }).is_ok()
}

const CAP: u64 = 16 * 1024 * 1024;
const SOURCE: &str = r#"
use std::relax::Host;
use test_crate::{note, Journal};
fn mix(x: u64, i: u64) -> u64 {
    let y = x.rotate_left(13) ^ i.rotate_right(7);
    y.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(31) ^ (y >> 29)
}
fn step(h: Host<Journal>, v: &mut [u64; 8], i: u64) -> u64 {
    let k = (i % 8) as usize;
    let a = mix(v[k], i);
    v[k] = a;
    note(h, a);
    let b = mix(a, v[(k + 3) % 8]) / (i | 1);
    v[(k + 5) % 8] ^= b;
    if b & 1 == 0 { note(h, b); mix(b, a) } else { b.rotate_left(9) }
}
pub fn run(h: Host<Journal>, n: u64) -> u64 {
    let mut v = [1u64, 2, 3, 4, 5, 6, 7, 8];
    let mut acc = 0u64;
    let mut i = 0;
    while i < n {
        acc = acc.rotate_left(3) ^ step(h, &mut v, i);
        i += 1;
    }
    note(h, acc ^ v[3]);
    acc
}
pub fn walk(h: Host<Journal>, n: u64) -> u64 {
    let mut v = [3u64; 8];
    let mut acc = n;
    let mut i = 0;
    while i < n {
        let a = mix(acc, i);
        let k = (a % 8) as usize + (i / 6) as usize * 8;
        v[k] = a;
        acc ^= v[(k + 1) % 8].rotate_left(11);
        if i % 3 == 0 { note(h, acc); }
        i += 1;
    }
    acc
}
"#;

/// (fuel consumed, how the call ended, what the host was told: notes, their fold)
fn measure(m: &Module, run: fn(Host<Journal>, u64) -> u64, fuel: u64, n: u64) -> (u64, String, u64, u64) {
    let mut journal = Journal { notes: 0, fold: 0 };
    let mut budget = Budget::new(fuel, CAP);
    let r = m.with_host(&mut journal, |h| m.call_with(&mut budget, || run(h, n))).unwrap();
    let how = match r {
        Ok(v) => format!("{:016x}", v),
        Err(BudgetError::Fuel { .. }) => "out of fuel".to_string(),
        Err(e) => format!("{:?}", e),
    };
    (budget.consumed_fuel(), how, journal.notes, journal.fold)
}

#[test]
fn fuel_chains() {
    let m = Module::load_source("chains", "chains.rs", SOURCE, &["test_crate"]).unwrap();
    let run: fn(Host<Journal>, u64) -> u64 = m.get("run").unwrap();
    // warm every function of the loop, then leave the background compiler time to install
    let mut journal = Journal { notes: 0, fold: 0 };
    m.with_host(&mut journal, |h| for _ in 0..300 { std::hint::black_box(run(h, 60)); }).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    const N: u64 = 9;
    let whole = measure(&m, run, 100_000_000, N);
    let (mut fold, mut ended) = (0u64, 0u64);
    let mut stops: Vec<(u64, u64)> = Vec::new();
    for fuel in 1..=whole.0 + 1 {
        let r = measure(&m, run, fuel, N);
        if r.1 == "out of fuel" { ended += 1; }
        if !stops.contains(&(r.2, r.3)) { stops.push((r.2, r.3)); }
        let text = format!("{} {} {} {} {:016x}", fuel, r.0, r.1, r.2, r.3);
        for b in text.bytes() { fold = (fold ^ b as u64).wrapping_mul(0x100000001b3); }
    }
    println!("out whole run: fuel {} result {} notes {} fold {:016x}", whole.0, whole.1, whole.2, whole.3);
    println!("out budgets 1..={}: {} ended on fuel, at {} different points, all of it {:016x}", whole.0 + 1, ended, stops.len(), fold);
    // the same for `walk`, which ends in a panic when its budget lets it get there
    let walk: fn(Host<Journal>, u64) -> u64 = m.get("walk").unwrap();
    for _ in 0..300 { let mut j = Journal { notes: 0, fold: 0 }; let _ = m.with_host(&mut j, |h| m.call_with(&mut Budget::new(100_000_000, CAP), || walk(h, 6))); }
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let whole = measure(&m, walk, 100_000_000, 8);
    let (mut fold, mut ended) = (0u64, 0u64);
    for fuel in 1..=whole.0 + 1 {
        let r = measure(&m, walk, fuel, 8);
        if r.1 == "out of fuel" { ended += 1; }
        let text = format!("{} {} {} {} {:016x}", fuel, r.0, r.1, r.2, r.3);
        for b in text.bytes() { fold = (fold ^ b as u64).wrapping_mul(0x100000001b3); }
    }
    println!("out walk: fuel {} notes {} ends with {}", whole.0, whole.2, whole.1);
    println!("out walk budgets 1..={}: {} ended on fuel, all of it {:016x}", whole.0 + 1, ended, fold);
    m.unload();
}

// O3 keeps every guarantee a twin has below it. run.sh runs this file with O3 off (RELAX_O3=0)
// and with O3 early, waits in the second run until the twins' O3 code is installed, and requires
// both runs' lines to be equal: the result and fuel charge of each call, a Budget one unit short
// stopping it, the bounds of the module's memory (outside its arena an access ends the call or
// misses the host; inside, what was written is read back), and indirect calls (a forged pointer
// never reaches host code). Every twin the test calls is warmed, `call_at` included (with the
// module's own fn value), so the forged call runs its O3 code; run.sh requires an O3 install of
// each. `zero_const` holds the constant-bound zeroing loops O3 turns into a memset call in host
// code: in a twin its stores stay masked. (--allow-unchecked-modules: the forged accesses need
// `unsafe`.)
use std::relax::{guard, Budget, BudgetError, Module};

static mut SECRET: u64 = 0x1234_5678;

const M: &str = r#"
pub fn mix(n: u64) -> u64 { let mut x = 1u64; let mut i = 0; while i < n { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); i += 1; } x }
pub fn fill(n: u64) -> u64 { let mut v = vec![0u8; n as usize]; for (i, b) in v.iter_mut().enumerate() { *b = i as u8; } v.iter().map(|b| *b as u64).sum() }
pub fn zero(n: u64) -> u64 { let mut a = [1u64; 64]; for x in a[..(n as usize).min(64)].iter_mut() { *x = 0; } a.iter().sum() }
pub fn zero_const(n: u64) -> u64 {
    let mut a = [n; 64];
    for i in 0..64 { a[i] = 0; }
    let mut b = [n; 32];
    let p = b.as_mut_ptr();
    let mut i = 0;
    while i < 32 { unsafe { *p.add(i) = 0; } i += 1; }
    std::hint::black_box(&a).iter().sum::<u64>() + std::hint::black_box(&b).iter().sum::<u64>() + n
}
pub fn table(i: u64) -> u64 { let fs: [fn(u64) -> u64; 3] = [|x| x + 1, |x| x * 2, |x| x ^ 7]; std::hint::black_box(fs)[(i % 3) as usize](i) }
pub fn own_addr() -> u64 { static mut OWN: u64 = 0; unsafe { &raw const OWN as u64 } }
pub fn read_at(a: u64) -> u64 { unsafe { *(a as *const u64) } }
pub fn write_at(a: u64, v: u64) -> u64 { unsafe { *(a as *mut u64) = v; } 1 }
pub fn call_at(a: u64) -> u64 { let f: fn() -> u64 = unsafe { core::mem::transmute(a as usize) }; f() }
pub fn seven() -> u64 { 7 }
pub fn seven_fn() -> u64 { std::hint::black_box(seven as fn() -> u64) as usize as u64 }
"#;

fn host_value() -> u64 {
    0x5eed
}

#[test]
fn twins_keep_their_guarantees_at_o3() {
    let m = Module::load_source("tiers", "tiers.rs", M, &[]).unwrap();
    let mix: fn(u64) -> u64 = m.get("mix").unwrap();
    let fill: fn(u64) -> u64 = m.get("fill").unwrap();
    let zero: fn(u64) -> u64 = m.get("zero").unwrap();
    let zero_const: fn(u64) -> u64 = m.get("zero_const").unwrap();
    let table: fn(u64) -> u64 = m.get("table").unwrap();
    let own_addr: fn() -> u64 = m.get("own_addr").unwrap();
    let read_at: fn(u64) -> u64 = m.get("read_at").unwrap();
    let write_at: fn(u64, u64) -> u64 = m.get("write_at").unwrap();
    let call_at: fn(u64) -> u64 = m.get("call_at").unwrap();
    let seven_fn: fn() -> u64 = m.get("seven_fn").unwrap();
    let own = own_addr();
    let seven = seven_fn();
    let warm = || {
        for _ in 0..100 {
            mix(50);
            fill(100);
            zero(10);
            zero_const(3);
            table(4);
            call_at(seven);
            write_at(own, 5);
            read_at(own);
        }
    };
    for _ in 0..30 {
        warm();
    }
    // with O3 on, run.sh names the JIT's O3 progress file: warm until no O3 build is pending and
    // one was installed at least
    if let Ok(progress) = std::env::var("RELAX_O3_PROGRESS") {
        loop {
            let line = std::fs::read_to_string(&progress).unwrap_or_default();
            let n: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if n.len() == 2 && n[0] == 0 && n[1] > 0 {
                break;
            }
            warm();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    // (the module's fn value printed by name: its address is no behaviour of the code)
    let calls: [(&str, &str, fn(u64) -> u64, u64); 6] =
        [("mix", "1000", mix, 1000), ("fill", "3000", fill, 3000), ("zero", "40", zero, 40), ("zero_const", "5", zero_const, 5), ("table", "5", table, 5), ("call_at", "seven", call_at, seven)];
    for (name, shown, f, arg) in calls {
        let mut budget = Budget::new(10_000_000, 1 << 24);
        let r = m.call_with(&mut budget, || f(arg));
        let fuel = budget.consumed_fuel();
        println!("out {name}({shown}): {r:?}, fuel {fuel}");
        let mut short = Budget::new(fuel - 1, 1 << 24);
        println!("out {name} one unit short stops: {}", matches!(m.call_with(&mut short, || f(arg)), Err(BudgetError::Fuel { .. })));
    }
    let secret = &raw const SECRET as u64;
    let _ = guard(1000, || write_at(secret, 1));
    println!("out host write missed the host: {}", unsafe { SECRET } == 0x1234_5678);
    println!("out host read missed the host: {}", guard(1000, || read_at(secret)).map_or(true, |v| v != 0x1234_5678));
    println!("out own memory: {:?}", guard(1000, || { write_at(own, 77); read_at(own) }));
    println!("out unmapped offset ends the call: {}", guard(1000, || read_at(0)).is_err());
    let forged = guard(1000, || call_at(host_value as fn() -> u64 as u64));
    println!("out forged pointer contained: {}", forged.map_or(true, |v| v != host_value()));
}

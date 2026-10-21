// KNOWN FAILING (baseline tier): the payload message of an integer division by zero is "explicit panic" under
// Relax, rustc says "attempt to divide by zero". Found by diff_fast 2026-10-20. Move to ../diff/ when fixed.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }


fn work() {
    let r = std::panic::catch_unwind(|| { let d = std::hint::black_box(0); 10 / d });
    let e = r.unwrap_err();
    d!("{}", e.downcast_ref::<&str>().copied().unwrap_or("?"));
}

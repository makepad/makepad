// The file form outlives a test's faults: a hang (the stall watchdog unwinds it), a null read and a
// division by zero each fail their test, and the next test runs (rustc's build dies at the null read).
// corpus: fails hang segv div_zero
// corpus: env RELAX_WATCHDOG_MS=2000
fn spin() -> u64 {
    let mut i = 0u64;
    loop {
        i += 1;
        if i == 0 {
            break;
        }
    }
    i
}

fn null_read() -> u64 {
    let p = 16 as *const u64;
    unsafe { *p }
}

fn div(a: i32, b: i32) -> i32 {
    a / b
}

#[test]
fn hang() {
    println!("{}", spin());
}

#[test]
fn segv() {
    println!("{}", null_read());
}

#[test]
fn div_zero() {
    println!("{}", div(1, 0));
}

#[test]
fn after_crashes_still_running() {
    assert_eq!(div(7, 2), 3);
}

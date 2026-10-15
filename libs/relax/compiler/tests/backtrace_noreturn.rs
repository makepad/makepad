// Backtraces through calls that never return: `fail` ends in a panic call, so the return
// address of that call is the first byte of the next function. Relax's report must name
// `fail` at line 9 and `middle` at line 15 (return addresses looked up at ret-1).
// Run with `makepad test`; the test panics on purpose (rustc's test harness reports the
// same panic message).

#[inline(never)]
fn fail(x: u64) -> u64 {
    if x > 100 { return x; } panic!("no return {}", x)
}

#[inline(never)]
fn middle(x: u64) -> u64 {
    let y = x * 2;
    fail(y) + 1
}

#[inline(never)]
fn after(x: u64) -> u64 {
    x + 3
}

#[test]
#[should_panic]
fn noreturn_backtrace() {
    let r = middle(after(4));
    println!("{}", r);
}

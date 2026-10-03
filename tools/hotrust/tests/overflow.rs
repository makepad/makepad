// Stack overflow in JIT code is reported as "stack overflow" (guard page, recovered on the
// alternate signal stack) and the next test still runs. Intentionally fails one test.
fn deep(n: u64) -> u64 {
    let mut pad = [0u64; 32];
    pad[(n % 32) as usize] = n;
    deep(n + 1) + pad[3]
}
#[test]
fn overflow() {
    println!("{}", deep(0));
}
#[test]
fn after() {
    println!("still running");
}

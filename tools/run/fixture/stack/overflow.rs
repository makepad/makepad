// A runaway recursion on a program stack faults on its guard and is reported as a stack overflow
// (program_stack.sh runs it with MAKEPAD_RUN_STACK_GB=1).
fn deep(n: u64) -> u64 {
    let mut pad = [0u64; 32];
    pad[(n % 32) as usize] = n;
    deep(n + 1) + std::hint::black_box(pad)[3]
}

#[test]
fn overflow() {
    println!("{}", deep(0));
}

// G4 / 3.17: a patch is all or nothing. v1 edits kernel (valid) and adds a type error to a fn that
// was never called: the whole save is refused and the running kernel stays. v2 fixes the error:
// both edits apply.
fn kernel(x: u64) -> u64 {
    x * 5 + 3
}

fn never_called(n: u64) -> u64 {
    let s: String = n;
    s.len() as u64
}

#[test]
fn run() {
    let mut s = 0u64;
    for i in 0..100 {
        s = s.wrapping_mul(31).wrapping_add(kernel(i));
    }
    println!("out {}", s);
}

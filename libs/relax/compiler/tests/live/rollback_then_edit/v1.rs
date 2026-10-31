// 3.18 / M7: a patch whose code panics is rolled back (code, file version and consts), and the
// next edit applies against the running version.
//  v1: `a` indexes out of bounds and `LIMIT` changes: the run panics, the patch group rolls back
//      (a and LIMIT return to v0).
//  v2: only `b` changes against v0: `a` and LIMIT stay as v0 had them.
const LIMIT: u64 = 30;

fn a(x: u64) -> u64 {
    let t = [1u64, 2, 3];
    t[(x + 9) as usize] + LIMIT
}

fn b(x: u64) -> u64 {
    x * 2
}

#[test]
fn run() {
    println!("out {} {}", a(1), b(2));
}
